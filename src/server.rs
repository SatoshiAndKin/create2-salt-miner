use std::{
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
    pub max_runtime_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
struct NormalizedMineRequest {
    factory: String,
    caller: String,
    codehash: String,
    worksize: u32,
    zeros: usize,
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
    let connection = Connection::open(&config.cache_path)
        .wrap_err_with(|| format!("failed to open cache at {}", config.cache_path.display()))?;
    init_cache(&connection).wrap_err("failed to initialize cache")?;
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
            let response = mining_response(outcome, normalized);
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

fn mining_response(
    outcome: Option<crate::miner::MiningOutcome>,
    request: &NormalizedMineRequest,
) -> MineResponse {
    match outcome {
        Some(outcome) => MineResponse {
            cache_hit: false,
            found: true,
            salt: Some(format!("0x{}", hex::encode(outcome.salt))),
            address: Some(outcome.address.to_string()),
            score: Some(outcome.score),
            runtime_ms: outcome.runtime.as_millis(),
        },
        None => MineResponse {
            cache_hit: false,
            found: false,
            salt: None,
            address: None,
            score: None,
            runtime_ms: request
                .max_runtime_secs
                .map_or(0, |secs| u128::from(secs) * u128::from(1_000_u16)),
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
            once: true,
            abi: false,
        })
    }

    fn stop_mode(&self) -> MiningStop {
        self.max_runtime_secs
            .map(|secs| MiningStop::Timed(std::time::Duration::from_secs(secs)))
            .unwrap_or(MiningStop::FirstMatch)
    }
}

fn normalize_request(request: MineRequest) -> Result<NormalizedMineRequest> {
    let max_runtime_secs = match request.max_runtime_secs {
        Some(0) => return Err(eyre!("max_runtime_secs must be greater than zero")),
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
        max_runtime_secs,
    })
}

fn init_cache(connection: &Connection) -> rusqlite::Result<()> {
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
