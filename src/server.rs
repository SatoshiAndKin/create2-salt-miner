use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use alloy_primitives::hex;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use eyre::{Context, OptionExt, Result, eyre};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, sync::Notify};
use url::{Host, Url};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    AppConfig, DEFAULT_FACTORY, decode_fixed,
    miner::{MiningStop, mine_once},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub cache_path: PathBuf,
}

#[derive(Debug, Clone)]
struct ServerState {
    cache: Arc<Mutex<Connection>>,
    jobs_changed: Arc<Notify>,
}

#[derive(Debug, Serialize, ToSchema)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MineRequest {
    #[schema(example = "0x0000000000FFe8B47B3e2130213B802212439497")]
    pub factory: Option<String>,
    #[schema(example = "0x0000000000000000000000000000000000000000")]
    pub caller: String,
    #[schema(example = "0x64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107")]
    pub codehash: String,
    pub worksize: Option<u32>,
    pub zeros: Option<usize>,
    pub min_runtime_secs: Option<u64>,
    /// Mining seconds before returning the best candidate, even below target.
    /// Checked at batch boundaries; excludes setup, queue, and network time.
    pub max_runtime_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
struct NormalizedMineRequest {
    factory: String,
    caller: String,
    codehash: String,
    worksize: u32,
    zeros: usize,
    min_runtime_secs: Option<u64>,
    max_runtime_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MineResponse {
    pub cache_hit: bool,
    pub found: bool,
    pub salt: Option<String>,
    pub address: Option<String>,
    pub score: Option<usize>,
    pub runtime_ms: u128,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ErrorResponse {
    error: String,
}

#[derive(OpenApi)]
#[openapi(
    paths(health, mine),
    components(schemas(HealthResponse, MineRequest, MineResponse, ErrorResponse)),
    tags((name = "mining", description = "CREATE2 salt mining"))
)]
struct ApiDoc;

pub async fn start_server(config: ServerConfig) -> Result<()> {
    let mut connection = Connection::open(&config.cache_path)
        .wrap_err_with(|| format!("failed to open cache at {}", config.cache_path.display()))?;
    init_cache(&mut connection).wrap_err("failed to initialize cache")?;
    requeue_running_jobs(&connection).wrap_err("failed to requeue interrupted jobs")?;

    let state = ServerState {
        cache: Arc::new(Mutex::new(connection)),
        jobs_changed: Arc::new(Notify::new()),
    };
    tokio::spawn(mining_worker(state.clone()));

    let app = Router::new()
        .route("/health", get(health))
        .route("/mine", post(mine))
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .wrap_err("failed to parse server address")?;
    let listener = TcpListener::bind(addr)
        .await
        .wrap_err_with(|| format!("failed to bind {addr}"))?;
    println!("Listening on http://{addr}");
    axum::serve(listener, app).await.wrap_err("server failed")
}

pub async fn mine_remote(remote_server: &str, request: MineRequest) -> Result<MineResponse> {
    let remote_server = remote_server.to_owned();
    tokio::task::spawn_blocking(move || mine_remote_blocking(&remote_server, &request))
        .await
        .wrap_err("remote mining request failed to join")?
}

fn mine_remote_blocking(remote_server: &str, request: &MineRequest) -> Result<MineResponse> {
    let endpoint = remote_mine_endpoint(remote_server)?;
    let body = serde_json::to_vec(request).wrap_err("failed to serialize mining request")?;
    let (connection_addr, host_header) = remote_connection_parts(&endpoint)?;

    let mut stream = std::net::TcpStream::connect(&connection_addr)
        .wrap_err_with(|| format!("failed to connect to remote mining server at {endpoint}"))?;
    let request_head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.path(),
        host_header,
        body.len()
    );

    stream
        .write_all(request_head.as_bytes())
        .wrap_err("failed to write remote mining request headers")?;
    stream
        .write_all(&body)
        .wrap_err("failed to write remote mining request body")?;
    stream
        .flush()
        .wrap_err("failed to flush remote mining request")?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .wrap_err("failed to read remote mining response")?;
    let (status, body) = parse_http_response(&response)?;

    if !(200..300).contains(&status) {
        let message = serde_json::from_slice::<ErrorResponse>(&body)
            .map(|response| response.error)
            .or_else(|_| String::from_utf8(body.clone()))
            .unwrap_or_else(|_| "remote server returned an invalid error body".to_owned());
        return Err(eyre!(
            "remote mining server returned HTTP {status}: {message}"
        ));
    }

    serde_json::from_slice(&body).wrap_err("failed to deserialize remote mining response")
}

fn remote_mine_endpoint(remote_server: &str) -> Result<Url> {
    let mut endpoint =
        Url::parse(remote_server).wrap_err("failed to parse remote_server as a URL")?;

    if endpoint.scheme() != "http" {
        return Err(eyre!(
            "remote_server must use http:// because the built-in mining server does not serve TLS"
        ));
    }

    endpoint
        .host()
        .ok_or_eyre("remote_server URL must include a host")?;
    endpoint.set_query(None);
    endpoint.set_fragment(None);

    let path = endpoint.path().trim_end_matches('/');
    let mine_path = if path.is_empty() {
        "/mine".to_owned()
    } else if path.ends_with("/mine") {
        path.to_owned()
    } else {
        format!("{path}/mine")
    };
    endpoint.set_path(&mine_path);

    Ok(endpoint)
}

fn remote_connection_parts(endpoint: &Url) -> Result<(String, String)> {
    let port = endpoint
        .port_or_known_default()
        .ok_or_eyre("remote_server URL must include a port")?;
    let host = endpoint
        .host()
        .ok_or_eyre("remote_server URL must include a host")?;

    let (connection_addr, host_header_base) = match host {
        Host::Domain(domain) => (format!("{domain}:{port}"), domain.to_owned()),
        Host::Ipv4(address) => (format!("{address}:{port}"), address.to_string()),
        Host::Ipv6(address) => (format!("[{address}]:{port}"), format!("[{address}]")),
    };
    let host_header = if endpoint.port().is_some() {
        format!("{host_header_base}:{port}")
    } else {
        host_header_base
    };

    Ok((connection_addr, host_header))
}

fn parse_http_response(response: &[u8]) -> Result<(u16, Vec<u8>)> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_eyre("remote server returned an invalid HTTP response")?;
    let header = std::str::from_utf8(&response[..header_end])
        .wrap_err("remote server returned non-UTF-8 HTTP headers")?;
    let body = response[(header_end + 4)..].to_vec();

    let mut lines = header.lines();
    let status_line = lines
        .next()
        .ok_or_eyre("remote server returned an empty HTTP response")?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_eyre("remote server returned an invalid HTTP status line")?
        .parse()
        .wrap_err("remote server returned a non-numeric HTTP status")?;

    Ok((status, body))
}

#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Server is healthy", body = HealthResponse))
)]
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[utoipa::path(
    post,
    path = "/mine",
    request_body = MineRequest,
    responses(
        (status = 200, description = "Mining completed", body = MineResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Mining failed", body = ErrorResponse)
    )
)]
async fn mine(State(state): State<ServerState>, Json(request): Json<MineRequest>) -> Response {
    match mine_inner(state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => (
            error.status,
            Json(ErrorResponse {
                error: error.message,
            }),
        )
            .into_response(),
    }
}

#[derive(Debug)]
struct ServerError {
    status: StatusCode,
    message: String,
}

impl ServerError {
    fn bad_request(error: eyre::Report) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: error.to_string(),
        }
    }

    fn internal(error: eyre::Report) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

async fn mine_inner(
    state: ServerState,
    request: MineRequest,
) -> std::result::Result<MineResponse, ServerError> {
    let normalized = normalize_request(request).map_err(ServerError::bad_request)?;
    normalized
        .to_app_config()
        .map_err(ServerError::bad_request)?;
    let request_key = serde_json::to_string(&normalized)
        .map_err(|error| ServerError::internal(eyre!("failed to serialize request: {error}")))?;

    if let Some(mut response) =
        get_cached_response(&state, &request_key).map_err(ServerError::internal)?
    {
        response.cache_hit = true;
        return Ok(response);
    }

    enqueue_job(&state, &request_key, &normalized).map_err(ServerError::internal)?;
    state.jobs_changed.notify_one();

    wait_for_mining_response(&state, &request_key).await
}

async fn mining_worker(state: ServerState) {
    loop {
        match run_next_job(&state).await {
            Ok(true) => continue,
            Ok(false) => state.jobs_changed.notified().await,
            Err(error) => {
                eprintln!("mining worker failed: {error:?}");
                state.jobs_changed.notified().await;
            }
        }
    }
}

async fn run_next_job(state: &ServerState) -> Result<bool> {
    let Some((request_key, normalized)) = claim_next_job(state)? else {
        return Ok(false);
    };

    let result = run_claimed_job(state, &request_key, &normalized).await;
    if let Err(error) = &result
        && let Err(mark_error) = mark_job_failed(state, &request_key, &error.to_string())
    {
        eprintln!("failed to mark mining job failed: {mark_error:?}");
    }
    state.jobs_changed.notify_waiters();
    result.map(|()| true)
}

async fn run_claimed_job(
    state: &ServerState,
    request_key: &str,
    normalized: &NormalizedMineRequest,
) -> Result<()> {
    let config = normalized.to_app_config()?;
    let stop = normalized.stop_mode();
    let outcome = tokio::task::spawn_blocking(move || mine_once(config, stop))
        .await
        .wrap_err("mining task failed to join")?;

    match outcome {
        Ok(outcome) => {
            let response = mining_response(outcome);
            insert_cached_response(state, request_key, normalized, &response)?;
            mark_job_succeeded(state, request_key)?;
        }
        Err(error) => mark_job_failed(state, request_key, &error.to_string())?,
    }

    Ok(())
}

async fn wait_for_mining_response(
    state: &ServerState,
    request_key: &str,
) -> std::result::Result<MineResponse, ServerError> {
    loop {
        let changed = state.jobs_changed.notified();

        if let Some(mut response) =
            get_cached_response(state, request_key).map_err(ServerError::internal)?
        {
            response.cache_hit = true;
            return Ok(response);
        }

        if let Some(error) = get_job_error(state, request_key).map_err(ServerError::internal)? {
            return Err(ServerError::internal(eyre!(error)));
        }

        changed.await;
    }
}

fn mining_response(run: crate::miner::MiningRun) -> MineResponse {
    match run.outcome {
        Some(outcome) => MineResponse {
            cache_hit: false,
            found: true,
            salt: Some(format!("0x{}", hex::encode(outcome.salt))),
            address: Some(outcome.address.to_string()),
            score: Some(outcome.score),
            runtime_ms: run.runtime.as_millis(),
        },
        None => MineResponse {
            cache_hit: false,
            found: false,
            salt: None,
            address: None,
            score: None,
            runtime_ms: run.runtime.as_millis(),
        },
    }
}

impl NormalizedMineRequest {
    fn to_app_config(&self) -> Result<AppConfig> {
        Ok(AppConfig {
            factory: decode_fixed(&self.factory, "factory")?,
            caller: decode_fixed(&self.caller, "caller")?,
            codehash: decode_fixed(&self.codehash, "codehash")?,
            worksize: self.worksize,
            zeros: self.zeros,
            one: true,
            abi: false,
            min_runtime_secs: self.min_runtime_secs,
            max_runtime_secs: self.max_runtime_secs,
        })
    }

    fn stop_mode(&self) -> MiningStop {
        MiningStop::from_limits(self.min_runtime_secs, self.max_runtime_secs)
    }
}

fn normalize_request(request: MineRequest) -> Result<NormalizedMineRequest> {
    let min_runtime_secs = match request.min_runtime_secs {
        Some(0) => return Err(eyre!("min_runtime_secs must be greater than zero")),
        other => other,
    };

    Ok(NormalizedMineRequest {
        factory: request
            .factory
            .unwrap_or_else(|| DEFAULT_FACTORY.to_owned()),
        caller: request.caller,
        codehash: request.codehash,
        worksize: request.worksize.unwrap_or(0x4400000_u32),
        zeros: request.zeros.unwrap_or(6_usize),
        min_runtime_secs,
        max_runtime_secs: request.max_runtime_secs,
    })
}

fn init_cache(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    let connection = &transaction;
    connection.execute(
        "CREATE TABLE IF NOT EXISTS mine_cache (
            request_key TEXT PRIMARY KEY,
            factory TEXT NOT NULL,
            caller TEXT NOT NULL,
            codehash TEXT NOT NULL,
            worksize INTEGER NOT NULL,
            zeros INTEGER NOT NULL,
            max_runtime_secs INTEGER,
            response_json TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        [],
    )?;
    connection.execute(
        "CREATE TABLE IF NOT EXISTS mine_jobs (
            request_key TEXT PRIMARY KEY,
            request_json TEXT NOT NULL,
            status TEXT NOT NULL,
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )",
        [],
    )?;
    migrate_request_keys(connection)?;
    transaction.commit()?;
    Ok(())
}

// Both older layouts had one optional limit. Only unlimited requests retained
// the same mining behavior across every version of those layouts.
fn migrated_request_key(key: &str) -> Result<Option<String>> {
    let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(key) else {
        return Ok(None);
    };
    if fields.len() != 6
        || !["factory", "caller", "codehash", "worksize", "zeros"]
            .iter()
            .all(|field| fields.contains_key(*field))
        || !["min_runtime_secs", "max_runtime_secs"]
            .iter()
            .any(|field| fields.get(*field).is_some_and(serde_json::Value::is_null))
    {
        return Ok(None);
    }
    let request: NormalizedMineRequest =
        serde_json::from_str(key).wrap_err("invalid stored unlimited request key")?;
    Ok(Some(serde_json::to_string(&request)?))
}

fn migrate_request_keys(connection: &Connection) -> Result<()> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut statement = connection
        .prepare("SELECT request_key FROM mine_cache UNION SELECT request_key FROM mine_jobs")?;
    for key in statement.query_map([], |row| row.get::<_, String>(0))? {
        let key = key?;
        if let Some(current) = migrated_request_key(&key)? {
            groups.entry(current).or_default().push(key);
        }
    }
    drop(statement);
    for (current, mut keys) in groups {
        // Visit the current key first so it wins ties and existing cache results
        // always take precedence over results from an older layout.
        keys.insert(0, current.clone());
        migrate_request_group(connection, &current, &keys)?;
    }
    Ok(())
}

fn migrate_request_group(connection: &Connection, current: &str, keys: &[String]) -> Result<()> {
    struct StoredJob {
        key: String,
        request_json: String,
        status: String,
        error: Option<String>,
        created_at: i64,
        updated_at: i64,
    }

    let mut cached: Option<(&str, i64)> = None;
    let mut jobs = Vec::new();
    for key in keys {
        let created_at: Option<i64> = connection
            .query_row(
                "SELECT created_at FROM mine_cache WHERE request_key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(created_at) = created_at
            && cached
                .is_none_or(|(saved_key, saved_at)| saved_key != current && created_at > saved_at)
        {
            cached = Some((key, created_at));
        }
        let job = connection
            .query_row(
                "SELECT request_json, status, error, created_at, updated_at
                 FROM mine_jobs WHERE request_key = ?1",
                [key],
                |row| {
                    Ok(StoredJob {
                        key: key.clone(),
                        request_json: row.get(0)?,
                        status: row.get(1)?,
                        error: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        if let Some(job) = job {
            let request: NormalizedMineRequest = serde_json::from_str(&job.request_json)
                .wrap_err("invalid job payload during request key migration")?;
            if serde_json::to_string(&request)? != current {
                return Err(eyre!("job payload does not match its stored request key"));
            }
            jobs.push(job);
        }
    }

    if let Some((saved_key, _)) = cached {
        for key in keys.iter().filter(|key| key.as_str() != saved_key) {
            connection.execute("DELETE FROM mine_cache WHERE request_key = ?1", [key])?;
        }
        connection.execute(
            "UPDATE mine_cache SET request_key = ?1 WHERE request_key = ?2",
            params![current, saved_key],
        )?;
    }

    if let Some(selected) = jobs
        .iter()
        .max_by_key(|job| (job.key == current, job.updated_at))
    {
        let created_at = jobs.iter().map(|job| job.created_at).min().unwrap();
        let pending = jobs
            .iter()
            .any(|job| matches!(job.status.as_str(), "queued" | "running"));
        let status = if cached.is_some() {
            "succeeded"
        } else if pending {
            "queued"
        } else {
            &selected.status
        };
        let error = if cached.is_some() || pending {
            None
        } else {
            selected.error.as_deref()
        };
        let updated_at = if status != selected.status || error != selected.error.as_deref() {
            unix_timestamp()
        } else {
            selected.updated_at
        };
        for key in keys.iter().filter(|key| **key != selected.key) {
            connection.execute("DELETE FROM mine_jobs WHERE request_key = ?1", [key])?;
        }
        connection.execute(
            "UPDATE mine_jobs SET request_key = ?1, request_json = ?1, status = ?2,
             error = ?3, created_at = ?4, updated_at = ?5 WHERE request_key = ?6",
            params![current, status, error, created_at, updated_at, selected.key],
        )?;
    }
    Ok(())
}

fn requeue_running_jobs(connection: &Connection) -> rusqlite::Result<()> {
    let now = unix_timestamp();
    connection.execute(
        "UPDATE mine_jobs SET status = 'queued', error = NULL, updated_at = ?1 WHERE status = 'running'",
        [now],
    )?;
    Ok(())
}

fn get_cached_response(state: &ServerState, request_key: &str) -> Result<Option<MineResponse>> {
    let cache = state
        .cache
        .lock()
        .map_err(|_| eyre!("cache mutex poisoned"))?;
    let response_json: Option<String> = cache
        .query_row(
            "SELECT response_json FROM mine_cache WHERE request_key = ?1",
            [request_key],
            |row| row.get(0),
        )
        .optional()
        .wrap_err("failed to read cache")?;

    response_json
        .map(|json| serde_json::from_str(&json).wrap_err("failed to deserialize cached response"))
        .transpose()
}

fn insert_cached_response(
    state: &ServerState,
    request_key: &str,
    request: &NormalizedMineRequest,
    response: &MineResponse,
) -> Result<()> {
    let response_json =
        serde_json::to_string(response).wrap_err("failed to serialize cache response")?;
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .wrap_err("system clock is before Unix epoch")?
        .as_secs();
    let cache = state
        .cache
        .lock()
        .map_err(|_| eyre!("cache mutex poisoned"))?;
    cache
        .execute(
            "INSERT OR REPLACE INTO mine_cache (
                request_key,
                factory,
                caller,
                codehash,
                worksize,
                zeros,
                max_runtime_secs,
                response_json,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                request_key,
                request.factory,
                request.caller,
                request.codehash,
                request.worksize,
                request.zeros as i64,
                request.max_runtime_secs.map(|secs| secs as i64),
                response_json,
                created_at as i64
            ],
        )
        .wrap_err("failed to write cache")?;
    Ok(())
}

fn enqueue_job(
    state: &ServerState,
    request_key: &str,
    request: &NormalizedMineRequest,
) -> Result<()> {
    let request_json =
        serde_json::to_string(request).wrap_err("failed to serialize job request")?;
    let now = unix_timestamp();
    let cache = state
        .cache
        .lock()
        .map_err(|_| eyre!("cache mutex poisoned"))?;
    cache
        .execute(
            "INSERT INTO mine_jobs (
                request_key,
                request_json,
                status,
                error,
                created_at,
                updated_at
            ) VALUES (?1, ?2, 'queued', NULL, ?3, ?3)
            ON CONFLICT(request_key) DO UPDATE SET
                status = CASE WHEN mine_jobs.status = 'failed' THEN 'queued' ELSE mine_jobs.status END,
                error = CASE WHEN mine_jobs.status = 'failed' THEN NULL ELSE mine_jobs.error END,
                updated_at = CASE WHEN mine_jobs.status = 'failed' THEN excluded.updated_at ELSE mine_jobs.updated_at END",
            params![request_key, request_json, now],
        )
        .wrap_err("failed to enqueue mining job")?;
    Ok(())
}

fn claim_next_job(state: &ServerState) -> Result<Option<(String, NormalizedMineRequest)>> {
    let now = unix_timestamp();
    let cache = state
        .cache
        .lock()
        .map_err(|_| eyre!("cache mutex poisoned"))?;
    let job: Option<(String, String)> = cache
        .query_row(
            "SELECT request_key, request_json
             FROM mine_jobs
             WHERE status = 'queued'
             ORDER BY created_at, request_key
             LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .wrap_err("failed to read queued job")?;

    let Some((request_key, request_json)) = job else {
        return Ok(None);
    };

    cache
        .execute(
            "UPDATE mine_jobs SET status = 'running', error = NULL, updated_at = ?1 WHERE request_key = ?2",
            params![now, request_key],
        )
        .wrap_err("failed to mark mining job running")?;
    drop(cache);

    let request = serde_json::from_str(&request_json).wrap_err("failed to deserialize job")?;
    Ok(Some((request_key, request)))
}

fn mark_job_succeeded(state: &ServerState, request_key: &str) -> Result<()> {
    let now = unix_timestamp();
    let cache = state
        .cache
        .lock()
        .map_err(|_| eyre!("cache mutex poisoned"))?;
    cache
        .execute(
            "UPDATE mine_jobs SET status = 'succeeded', error = NULL, updated_at = ?1 WHERE request_key = ?2",
            params![now, request_key],
        )
        .wrap_err("failed to mark mining job succeeded")?;
    Ok(())
}

fn mark_job_failed(state: &ServerState, request_key: &str, error: &str) -> Result<()> {
    let now = unix_timestamp();
    let cache = state
        .cache
        .lock()
        .map_err(|_| eyre!("cache mutex poisoned"))?;
    cache
        .execute(
            "UPDATE mine_jobs SET status = 'failed', error = ?1, updated_at = ?2 WHERE request_key = ?3",
            params![error, now, request_key],
        )
        .wrap_err("failed to mark mining job failed")?;
    Ok(())
}

fn get_job_error(state: &ServerState, request_key: &str) -> Result<Option<String>> {
    let cache = state
        .cache
        .lock()
        .map_err(|_| eyre!("cache mutex poisoned"))?;
    cache
        .query_row(
            "SELECT error FROM mine_jobs WHERE request_key = ?1 AND status = 'failed'",
            [request_key],
            |row| row.get(0),
        )
        .optional()
        .wrap_err("failed to read job status")
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> MineRequest {
        MineRequest {
            factory: None,
            caller: format!("0x{}", "22".repeat(20)),
            codehash: format!("0x{}", "33".repeat(32)),
            worksize: Some(256),
            zeros: Some(6),
            min_runtime_secs: Some(30),
            max_runtime_secs: Some(20),
        }
    }

    fn state() -> ServerState {
        let mut connection = Connection::open_in_memory().unwrap();
        init_cache(&mut connection).unwrap();
        ServerState {
            cache: Arc::new(Mutex::new(connection)),
            jobs_changed: Arc::new(Notify::new()),
        }
    }

    fn unlimited_keys() -> Result<(String, String, String)> {
        let mut input = request();
        input.min_runtime_secs = None;
        input.max_runtime_secs = None;
        let key = serde_json::to_string(&normalize_request(input)?)?;
        Ok((
            key.replace(",\"max_runtime_secs\":null", ""),
            key.replace(",\"min_runtime_secs\":null", ""),
            key,
        ))
    }

    fn seed_cache(connection: &Connection, key: &str, created_at: i64) -> Result<()> {
        let request: NormalizedMineRequest = serde_json::from_str(key)?;
        let response = MineResponse {
            cache_hit: false,
            found: true,
            salt: Some(format!("0x{}", "44".repeat(32))),
            address: Some(format!("0x{}", "55".repeat(20))),
            score: Some(6),
            runtime_ms: created_at as u128,
        };
        connection.execute(
            "INSERT INTO mine_cache VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8)",
            params![
                key,
                request.factory,
                request.caller,
                request.codehash,
                request.worksize,
                request.zeros as i64,
                serde_json::to_string(&response)?,
                created_at
            ],
        )?;
        Ok(())
    }

    fn seed_job(connection: &Connection, key: &str, status: &str, timestamp: i64) -> Result<()> {
        connection.execute(
            "INSERT INTO mine_jobs VALUES (?1, ?1, ?2, ?3, ?4, ?4)",
            params![
                key,
                status,
                (status == "failed").then_some("device failed"),
                timestamp
            ],
        )?;
        Ok(())
    }

    #[tokio::test]
    async fn stored_unlimited_cache_is_reused_after_startup() -> Result<()> {
        let (old_minimum, old_maximum, current) = unlimited_keys()?;
        for old in [old_minimum, old_maximum] {
            let path = std::env::temp_dir().join(format!(
                "salty-cache-migration-{}-{}.db",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ));
            let mut connection = Connection::open(&path)?;
            init_cache(&mut connection)?;
            seed_cache(&connection, &old, 123)?;
            seed_job(&connection, &old, "running", 100)?;
            drop(connection);
            let mut connection = Connection::open(&path)?;
            init_cache(&mut connection)?;
            let state = ServerState {
                cache: Arc::new(Mutex::new(connection)),
                jobs_changed: Arc::new(Notify::new()),
            };
            let cached = get_cached_response(&state, &current)?.expect("migrated cache hit");
            assert_eq!(cached.runtime_ms, 123);
            let mut input = request();
            input.min_runtime_secs = None;
            input.max_runtime_secs = None;
            let response = mine_inner(state.clone(), input)
                .await
                .map_err(|e| eyre!(e.message))?;
            assert!(response.cache_hit);
            assert_eq!(response.salt, cached.salt);
            assert_eq!(response.address, cached.address);
            assert_eq!(response.score, cached.score);
            assert_eq!(response.runtime_ms, cached.runtime_ms);
            assert!(get_cached_response(&state, &old)?.is_none());
            assert!(claim_next_job(&state)?.is_none());
            drop(state);
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    #[test]
    fn stored_unlimited_jobs_resume_once_after_startup() -> Result<()> {
        let (old_minimum, old_maximum, current) = unlimited_keys()?;
        let mut connection = Connection::open_in_memory()?;
        init_cache(&mut connection)?;
        seed_job(&connection, &old_minimum, "running", 10)?;
        seed_job(&connection, &old_maximum, "queued", 20)?;
        seed_job(&connection, &current, "failed", 30)?;
        init_cache(&mut connection)?;
        init_cache(&mut connection)?;
        requeue_running_jobs(&connection)?;
        let state = ServerState {
            cache: Arc::new(Mutex::new(connection)),
            jobs_changed: Arc::new(Notify::new()),
        };
        let request: NormalizedMineRequest = serde_json::from_str(&current)?;
        enqueue_job(&state, &current, &request)?;
        let (key, claimed) = claim_next_job(&state)?.unwrap();
        assert_eq!(key, current);
        assert_eq!(claimed.stop_mode(), MiningStop::FirstMatch);
        assert!(claim_next_job(&state)?.is_none());
        let stored: (String, i64, Option<String>) = state.cache.lock().unwrap().query_row(
            "SELECT request_json, created_at, error FROM mine_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(stored, (current, 10, None));
        Ok(())
    }

    fn cache_snapshot(connection: &Connection) -> Result<Vec<Vec<rusqlite::types::Value>>> {
        let mut snapshot = Vec::new();
        for table in ["mine_cache", "mine_jobs"] {
            let mut statement =
                connection.prepare(&format!("SELECT * FROM {table} ORDER BY request_key"))?;
            let columns = statement.column_count();
            let rows = statement.query_map([], |row| {
                (0..columns)
                    .map(|column| row.get(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?;
            snapshot.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        Ok(snapshot)
    }

    #[test]
    fn stored_cache_collisions_preserve_results_and_finish_jobs() -> Result<()> {
        let (old_minimum, old_maximum, current) = unlimited_keys()?;
        for existing_current in [false, true] {
            let mut connection = Connection::open_in_memory()?;
            init_cache(&mut connection)?;
            seed_cache(&connection, &old_minimum, 20)?;
            seed_cache(&connection, &old_maximum, 30)?;
            if existing_current {
                seed_cache(&connection, &current, 10)?;
            }
            seed_job(&connection, &old_minimum, "running", 1)?;
            seed_job(&connection, &old_maximum, "queued", 2)?;
            seed_job(&connection, &current, "failed", 3)?;
            let expected: String = connection.query_row(
                "SELECT response_json FROM mine_cache WHERE request_key = ?1",
                [if existing_current {
                    &current
                } else {
                    &old_maximum
                }],
                |row| row.get(0),
            )?;
            init_cache(&mut connection)?;
            let cache: (String, String, i64) = connection.query_row(
                "SELECT request_key, response_json, created_at FROM mine_cache",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(
                cache,
                (
                    current.clone(),
                    expected,
                    if existing_current { 10 } else { 30 }
                )
            );
            let job: (String, String, Option<String>, i64) = connection.query_row(
                "SELECT request_json, status, error, created_at FROM mine_jobs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            assert_eq!(job, (current.clone(), "succeeded".to_owned(), None, 1));
            for table in ["mine_cache", "mine_jobs"] {
                let count: i64 =
                    connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?;
                assert_eq!(count, 1);
            }
            let before = cache_snapshot(&connection)?;
            init_cache(&mut connection)?;
            assert_eq!(cache_snapshot(&connection)?, before);
        }
        Ok(())
    }

    #[test]
    fn stored_terminal_jobs_keep_current_status_or_newest_old_status() -> Result<()> {
        let (old_minimum, old_maximum, current) = unlimited_keys()?;
        for existing_current in [false, true] {
            let mut connection = Connection::open_in_memory()?;
            init_cache(&mut connection)?;
            seed_job(&connection, &old_minimum, "succeeded", 10)?;
            seed_job(&connection, &old_maximum, "failed", 20)?;
            if existing_current {
                seed_job(&connection, &current, "succeeded", 5)?;
            }
            init_cache(&mut connection)?;
            let job: (String, String, Option<String>, i64, i64) = connection.query_row(
                "SELECT request_key, status, error, created_at, updated_at FROM mine_jobs",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
            let expected = if existing_current {
                (current.clone(), "succeeded".to_owned(), None, 5, 5)
            } else {
                (
                    current.clone(),
                    "failed".to_owned(),
                    Some("device failed".to_owned()),
                    10,
                    20,
                )
            };
            assert_eq!(job, expected);
        }
        Ok(())
    }

    #[test]
    fn stored_limited_and_unknown_requests_remain_unchanged() -> Result<()> {
        let (old_minimum, old_maximum, current) = unlimited_keys()?;
        let mut connection = Connection::open_in_memory()?;
        init_cache(&mut connection)?;
        for key in [
            old_minimum.replace("\"min_runtime_secs\":null", "\"min_runtime_secs\":30"),
            old_maximum.replace("\"max_runtime_secs\":null", "\"max_runtime_secs\":30"),
            current.replace("\"max_runtime_secs\":null", "\"max_runtime_secs\":0"),
            current,
            old_minimum.replace("\"min_runtime_secs\":null", "\"future_option\":null"),
        ] {
            seed_cache(&connection, &key, 10)?;
            seed_job(&connection, &key, "failed", 20)?;
        }
        seed_job(&connection, "not JSON", "queued", 30)?;
        let before = cache_snapshot(&connection)?;
        init_cache(&mut connection)?;
        assert_eq!(cache_snapshot(&connection)?, before);
        Ok(())
    }

    #[test]
    fn stored_key_migration_rolls_back_on_bad_payloads_and_write_errors() -> Result<()> {
        let (old_minimum, _, _) = unlimited_keys()?;
        for failure in ["mismatch", "invalid JSON", "write error"] {
            let mut connection = Connection::open_in_memory()?;
            init_cache(&mut connection)?;
            seed_cache(&connection, &old_minimum, 10)?;
            // The second group fails after the first group's cache has moved.
            let other = old_minimum.replace("\"zeros\":6", "\"zeros\":7");
            seed_job(&connection, &other, "queued", 20)?;
            match failure {
                "mismatch" => {
                    connection.execute("UPDATE mine_jobs SET request_json = ?1", [&old_minimum])?;
                }
                "invalid JSON" => {
                    connection.execute("UPDATE mine_jobs SET request_json = 'not JSON'", [])?;
                }
                _ => connection.execute_batch(
                    "CREATE TRIGGER reject_migration BEFORE UPDATE ON mine_jobs
                     BEGIN SELECT RAISE(ABORT, 'test write failure'); END;",
                )?,
            }
            let before = cache_snapshot(&connection)?;
            let error = init_cache(&mut connection).unwrap_err().to_string();
            assert!(
                error.contains(match failure {
                    "mismatch" => "job payload does not match",
                    "invalid JSON" => "invalid job payload",
                    _ => "test write failure",
                }),
                "{error}"
            );
            assert_eq!(cache_snapshot(&connection)?, before);
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a native Metal or OpenCL device"]
    async fn native_queued_maximum_returns_fallback_and_measured_time() -> Result<()> {
        let state = state();
        let mut input = request();
        input.worksize = Some(1_048_576);
        input.zeros = Some(21);
        input.min_runtime_secs = None;
        input.max_runtime_secs = Some(1);
        let request = normalize_request(input)?;
        let key = serde_json::to_string(&request)?;
        enqueue_job(&state, &key, &request)?;
        assert!(run_next_job(&state).await?);
        let response = get_cached_response(&state, &key)?.unwrap();
        assert!(response.found);
        assert!(response.score.unwrap() < 21);
        assert!(response.runtime_ms >= 1_000);
        let salt = decode_fixed::<32>(response.salt.as_deref().unwrap(), "salt")?;
        let address = alloy_primitives::Address::from_slice(&decode_fixed::<20>(
            &request.factory,
            "factory",
        )?)
        .create2(salt, decode_fixed::<32>(&request.codehash, "codehash")?);
        assert_eq!(response.address, Some(address.to_string()));
        assert_eq!(
            response.score,
            Some(address.iter().filter(|&&byte| byte == 0).count())
        );
        assert!(claim_next_job(&state)?.is_none());
        Ok(())
    }

    #[test]
    fn limits_survive_serialization_normalization_and_queue_restart() -> Result<()> {
        let wire = serde_json::to_value(request())?;
        assert_eq!(wire["min_runtime_secs"], 30);
        assert_eq!(wire["max_runtime_secs"], 20);
        let request = normalize_request(serde_json::from_value(wire)?)?;
        let state = state();
        let key = serde_json::to_string(&request)?;
        enqueue_job(&state, &key, &request)?;
        claim_next_job(&state)?.unwrap();
        requeue_running_jobs(&state.cache.lock().unwrap())?;
        let (claimed_key, claimed) = claim_next_job(&state)?.unwrap();
        assert_eq!(claimed_key, key);
        assert_eq!(claimed.min_runtime_secs, Some(30));
        assert_eq!(claimed.max_runtime_secs, Some(20));
        let config = claimed.to_app_config()?;
        assert_eq!(config.min_runtime_secs, Some(30));
        assert_eq!(config.max_runtime_secs, Some(20));
        assert_eq!(
            claimed.stop_mode(),
            MiningStop::from_limits(Some(30), Some(20))
        );
        Ok(())
    }

    #[test]
    fn cache_separates_both_limits_and_stores_actual_maximum() -> Result<()> {
        let state = state();
        let request = normalize_request(request())?;
        let key = serde_json::to_string(&request)?;
        let response = mining_response(crate::miner::MiningRun {
            outcome: None,
            runtime: std::time::Duration::from_millis(20_123),
        });
        insert_cached_response(&state, &key, &request, &response)?;
        assert_eq!(
            get_cached_response(&state, &key)?.unwrap().runtime_ms,
            20_123
        );
        for (minimum, maximum) in [
            (None, Some(20)),
            (Some(31), Some(20)),
            (Some(30), None),
            (Some(30), Some(21)),
        ] {
            let mut other = request.clone();
            other.min_runtime_secs = minimum;
            other.max_runtime_secs = maximum;
            assert!(get_cached_response(&state, &serde_json::to_string(&other)?)?.is_none());
        }
        let maximum: i64 = state.cache.lock().unwrap().query_row(
            "SELECT max_runtime_secs FROM mine_cache WHERE request_key = ?1",
            [&key],
            |row| row.get(0),
        )?;
        assert_eq!(maximum, 20);
        Ok(())
    }

    #[test]
    fn normalization_preserves_limit_validation_and_absent_limits() -> Result<()> {
        let mut input = request();
        input.min_runtime_secs = Some(0);
        assert_eq!(
            normalize_request(input.clone()).unwrap_err().to_string(),
            "min_runtime_secs must be greater than zero"
        );
        input.min_runtime_secs = None;
        input.max_runtime_secs = Some(0);
        assert_eq!(normalize_request(input.clone())?.max_runtime_secs, Some(0));
        input.max_runtime_secs = None;
        assert_eq!(
            normalize_request(input)?.stop_mode(),
            MiningStop::FirstMatch
        );
        let wire = serde_json::json!({"caller": "0x00", "codehash": "0x00"});
        let input: MineRequest = serde_json::from_value(wire)?;
        assert_eq!(input.min_runtime_secs, None);
        assert_eq!(input.max_runtime_secs, None);
        Ok(())
    }

    #[test]
    fn response_preserves_fallback_and_measured_runtime() {
        let response = mining_response(crate::miner::MiningRun {
            outcome: Some(crate::miner::MiningOutcome {
                salt: [0x44; 32],
                address: alloy_primitives::Address::from([0x55; 20]),
                score: 0,
            }),
            runtime: std::time::Duration::from_millis(20_123),
        });
        assert!(response.found);
        assert!(!response.cache_hit);
        assert_eq!(response.score, Some(0));
        assert_eq!(response.salt, Some(format!("0x{}", "44".repeat(32))));
        assert_eq!(response.address, Some(format!("0x{}", "55".repeat(20))));
        assert_eq!(response.runtime_ms, 20_123);
        let empty = mining_response(crate::miner::MiningRun {
            outcome: None,
            runtime: std::time::Duration::from_millis(20_456),
        });
        assert!(!empty.found);
        assert_eq!((empty.salt, empty.address, empty.score), (None, None, None));
        assert_eq!(empty.runtime_ms, 20_456);
    }

    #[test]
    fn api_schema_exposes_both_optional_limits() -> Result<()> {
        let schema = serde_json::to_value(ApiDoc::openapi())?;
        let request = &schema["components"]["schemas"]["MineRequest"];
        for limit in ["min_runtime_secs", "max_runtime_secs"] {
            assert!(request["properties"].get(limit).is_some());
            assert!(
                !request["required"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!(limit))
            );
        }
        Ok(())
    }

    #[test]
    fn remote_mine_endpoint_appends_mine_to_base_url() {
        let endpoint = remote_mine_endpoint("http://127.0.0.1:3000").unwrap();
        assert_eq!(endpoint.as_str(), "http://127.0.0.1:3000/mine");
    }

    #[test]
    fn remote_mine_endpoint_preserves_existing_base_path() {
        let endpoint = remote_mine_endpoint("http://example.com/salty/").unwrap();
        assert_eq!(endpoint.as_str(), "http://example.com/salty/mine");
    }

    #[test]
    fn remote_mine_endpoint_rejects_https() {
        let error = remote_mine_endpoint("https://example.com").unwrap_err();
        assert!(error.to_string().contains("must use http://"));
    }

    #[test]
    fn parse_http_response_returns_status_and_body() {
        let (status, body) =
            parse_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"{}");
    }
}
