use std::{
    io::{Read, Write},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
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
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
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
    cache: Arc<Database>,
    jobs_changed: Arc<Notify>,
}

const MINE_CACHE: TableDefinition<&str, &[u8]> = TableDefinition::new("mine_cache");
const MINE_JOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("mine_jobs");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredJob {
    request: NormalizedMineRequest,
    status: JobStatus,
    created_at: u64,
}

impl ServerState {
    async fn with_cache<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Database) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let cache = self.cache.clone();
        tokio::task::spawn_blocking(move || operation(&cache))
            .await
            .wrap_err("database task failed to join")?
    }
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
    let cache = tokio::task::spawn_blocking(move || open_cache(&config.cache_path))
        .await
        .wrap_err("database startup task failed to join")??;
    let state = ServerState {
        cache: Arc::new(cache),
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

    let key = request_key.clone();
    let jobs_changed = state.jobs_changed.clone();
    let cached = state
        .with_cache(move |cache| {
            if let Some(response) = get_cached_response(cache, &key)? {
                return Ok(Some(response));
            }
            enqueue_job(cache, &key, &normalized)?;
            // The blocking task continues if the client disconnects during the write.
            jobs_changed.notify_one();
            Ok(None)
        })
        .await
        .map_err(ServerError::internal)?;
    if let Some(mut response) = cached {
        response.cache_hit = true;
        return Ok(response);
    }

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
    let Some((request_key, normalized)) = state.with_cache(claim_next_job).await? else {
        return Ok(false);
    };

    let result = run_claimed_job(state, &request_key, &normalized).await;
    if let Err(error) = &result {
        let message = error.to_string();
        if let Err(mark_error) = state
            .with_cache(move |cache| mark_job_failed(cache, &request_key, message))
            .await
        {
            eprintln!("failed to mark mining job failed: {mark_error:?}");
        }
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

    let request_key = request_key.to_owned();
    state
        .with_cache(move |cache| match outcome {
            Ok(outcome) => complete_job(cache, &request_key, &mining_response(outcome)),
            Err(error) => mark_job_failed(cache, &request_key, error.to_string()),
        })
        .await?;

    Ok(())
}

async fn wait_for_mining_response(
    state: &ServerState,
    request_key: &str,
) -> std::result::Result<MineResponse, ServerError> {
    loop {
        let changed = state.jobs_changed.notified();

        let key = request_key.to_owned();
        let (response, error) = state
            .with_cache(move |cache| {
                Ok((
                    get_cached_response(cache, &key)?,
                    get_job_error(cache, &key)?,
                ))
            })
            .await
            .map_err(ServerError::internal)?;
        if let Some(mut response) = response {
            response.cache_hit = true;
            return Ok(response);
        }
        if let Some(error) = error {
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

fn open_cache(path: &std::path::Path) -> Result<Database> {
    let cache = Database::create(path)
        .wrap_err_with(|| format!("failed to open redb database at {}", path.display()))?;
    init_cache(&cache)?;
    Ok(cache)
}

fn init_cache(cache: &Database) -> Result<()> {
    let transaction = cache.begin_write()?;
    transaction.open_table(MINE_CACHE)?;
    {
        let mut jobs = transaction.open_table(MINE_JOBS)?;
        let mut interrupted = Vec::new();
        for entry in jobs.iter()? {
            let (key, value) = entry?;
            let mut job: StoredJob = serde_json::from_slice(value.value())?;
            if job.status == JobStatus::Running {
                job.status = JobStatus::Queued;
                interrupted.push((key.value().to_owned(), serde_json::to_vec(&job)?));
            }
        }
        for (key, job) in interrupted {
            jobs.insert(key.as_str(), job.as_slice())?;
        }
    }
    transaction
        .commit()
        .wrap_err("failed to initialize database")?;
    Ok(())
}

fn get_cached_response(cache: &Database, request_key: &str) -> Result<Option<MineResponse>> {
    let transaction = cache.begin_read()?;
    let results = transaction.open_table(MINE_CACHE)?;
    results
        .get(request_key)?
        .map(|value| {
            serde_json::from_slice(value.value()).wrap_err("failed to deserialize cached response")
        })
        .transpose()
}

fn enqueue_job(cache: &Database, request_key: &str, request: &NormalizedMineRequest) -> Result<()> {
    let transaction = cache.begin_write()?;
    {
        let mut jobs = transaction.open_table(MINE_JOBS)?;
        let stored = jobs
            .get(request_key)?
            .map(|value| serde_json::from_slice::<StoredJob>(value.value()))
            .transpose()?;
        let job = match stored {
            Some(mut job) if matches!(job.status, JobStatus::Failed(_)) => {
                job.status = JobStatus::Queued;
                job
            }
            Some(_) => return Ok(()),
            None => StoredJob {
                request: request.clone(),
                status: JobStatus::Queued,
                created_at: unix_timestamp(),
            },
        };
        jobs.insert(request_key, serde_json::to_vec(&job)?.as_slice())?;
    }
    transaction
        .commit()
        .wrap_err("failed to enqueue mining job")?;
    Ok(())
}

fn claim_next_job(cache: &Database) -> Result<Option<(String, NormalizedMineRequest)>> {
    let transaction = cache.begin_write()?;
    let mut next: Option<(String, StoredJob)> = None;
    {
        let mut jobs = transaction.open_table(MINE_JOBS)?;
        for entry in jobs.iter()? {
            let (key, value) = entry?;
            let job: StoredJob = serde_json::from_slice(value.value())?;
            if job.status == JobStatus::Queued
                && next.as_ref().is_none_or(|(saved_key, saved_job)| {
                    (job.created_at, key.value()) < (saved_job.created_at, saved_key.as_str())
                })
            {
                next = Some((key.value().to_owned(), job));
            }
        }
        let Some((key, job)) = next.as_mut() else {
            return Ok(None);
        };
        job.status = JobStatus::Running;
        jobs.insert(key.as_str(), serde_json::to_vec(job)?.as_slice())?;
    }
    transaction
        .commit()
        .wrap_err("failed to claim mining job")?;
    Ok(next.map(|(key, job)| (key, job.request)))
}

fn complete_job(cache: &Database, request_key: &str, response: &MineResponse) -> Result<()> {
    let transaction = cache.begin_write()?;
    {
        let mut jobs = transaction.open_table(MINE_JOBS)?;
        let mut job: StoredJob = serde_json::from_slice(
            jobs.get(request_key)?
                .ok_or_eyre("completed mining job is missing")?
                .value(),
        )?;
        job.status = JobStatus::Succeeded;
        let mut results = transaction.open_table(MINE_CACHE)?;
        results.insert(request_key, serde_json::to_vec(response)?.as_slice())?;
        jobs.insert(request_key, serde_json::to_vec(&job)?.as_slice())?;
    }
    // redb's default Immediate durability persists both changes before returning.
    transaction
        .commit()
        .wrap_err("failed to complete mining job")?;
    Ok(())
}

fn mark_job_failed(cache: &Database, request_key: &str, error: String) -> Result<()> {
    let transaction = cache.begin_write()?;
    {
        let mut jobs = transaction.open_table(MINE_JOBS)?;
        let mut job: StoredJob = serde_json::from_slice(
            jobs.get(request_key)?
                .ok_or_eyre("failed mining job is missing")?
                .value(),
        )?;
        job.status = JobStatus::Failed(error);
        jobs.insert(request_key, serde_json::to_vec(&job)?.as_slice())?;
    }
    transaction
        .commit()
        .wrap_err("failed to mark mining job failed")?;
    Ok(())
}

fn get_job_error(cache: &Database, request_key: &str) -> Result<Option<String>> {
    let transaction = cache.begin_read()?;
    let jobs = transaction.open_table(MINE_JOBS)?;
    let job = jobs
        .get(request_key)?
        .map(|value| serde_json::from_slice::<StoredJob>(value.value()))
        .transpose()?;
    Ok(job.and_then(|job| match job.status {
        JobStatus::Failed(error) => Some(error),
        _ => None,
    }))
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
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
        let cache = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .unwrap();
        init_cache(&cache).unwrap();
        ServerState {
            cache: Arc::new(cache),
            jobs_changed: Arc::new(Notify::new()),
        }
    }

    fn stored_job(cache: &Database, key: &str) -> Result<StoredJob> {
        let transaction = cache.begin_read()?;
        let jobs = transaction.open_table(MINE_JOBS)?;
        Ok(serde_json::from_slice(jobs.get(key)?.unwrap().value())?)
    }

    fn seed_job(
        cache: &Database,
        request: &NormalizedMineRequest,
        status: JobStatus,
        created_at: u64,
    ) -> Result<String> {
        let key = serde_json::to_string(request)?;
        let job = StoredJob {
            request: request.clone(),
            status,
            created_at,
        };
        let transaction = cache.begin_write()?;
        transaction
            .open_table(MINE_JOBS)?
            .insert(key.as_str(), serde_json::to_vec(&job)?.as_slice())?;
        transaction.commit()?;
        Ok(key)
    }

    #[test]
    fn queue_orders_by_creation_time_then_request_key() -> Result<()> {
        let state = state();
        let mut input = normalize_request(request())?;
        let mut expected = Vec::new();
        for (zeros, timestamp) in [(9, 20), (8, 10), (7, 10)] {
            input.zeros = zeros;
            let key = seed_job(&state.cache, &input, JobStatus::Queued, timestamp)?;
            expected.push((timestamp, key));
        }
        expected.sort();
        for (_, key) in expected {
            assert_eq!(claim_next_job(&state.cache)?.unwrap().0, key);
        }
        assert!(claim_next_job(&state.cache)?.is_none());
        Ok(())
    }

    #[test]
    fn concurrent_requests_enqueue_and_claim_one_job() -> Result<()> {
        let state = state();
        let request = normalize_request(request())?;
        let key = serde_json::to_string(&request)?;
        std::thread::scope(|scope| -> Result<()> {
            let workers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| enqueue_job(&state.cache, &key, &request)))
                .collect();
            for worker in workers {
                worker.join().unwrap()?;
            }
            let workers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| claim_next_job(&state.cache)))
                .collect();
            let mut claimed = Vec::new();
            for worker in workers {
                if let Some(job) = worker.join().unwrap()? {
                    claimed.push(job.0);
                }
            }
            assert_eq!(claimed, vec![key.clone()]);
            Ok(())
        })?;
        enqueue_job(&state.cache, &key, &request)?;
        assert!(claim_next_job(&state.cache)?.is_none());
        Ok(())
    }

    #[test]
    fn failed_jobs_retry_without_an_old_error_or_changed_queue_order() -> Result<()> {
        let state = state();
        let request = normalize_request(request())?;
        let key = seed_job(&state.cache, &request, JobStatus::Queued, 10)?;
        claim_next_job(&state.cache)?.unwrap();
        mark_job_failed(&state.cache, &key, "device failed".to_owned())?;
        assert_eq!(
            get_job_error(&state.cache, &key)?.as_deref(),
            Some("device failed")
        );
        assert!(claim_next_job(&state.cache)?.is_none());
        enqueue_job(&state.cache, &key, &request)?;
        assert!(get_job_error(&state.cache, &key)?.is_none());
        assert_eq!(stored_job(&state.cache, &key)?.created_at, 10);
        assert_eq!(claim_next_job(&state.cache)?.unwrap().0, key);
        assert!(claim_next_job(&state.cache)?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn client_disconnect_keeps_the_job_and_completed_results_are_reused() -> Result<()> {
        let state = state();
        let input = request();
        let key = serde_json::to_string(&normalize_request(input.clone())?)?;
        let enqueued = state.jobs_changed.notified();
        let client = tokio::spawn(mine_inner(state.clone(), input.clone()));
        enqueued.await;
        client.abort();
        assert!(client.await.unwrap_err().is_cancelled());
        assert_eq!(claim_next_job(&state.cache)?.unwrap().0, key);
        let response = mining_response(crate::miner::MiningRun {
            outcome: Some(crate::miner::MiningOutcome {
                salt: [0x44; 32],
                address: alloy_primitives::Address::from([0x55; 20]),
                score: 6,
            }),
            runtime: std::time::Duration::from_millis(20_123),
        });
        complete_job(&state.cache, &key, &response)?;
        let cached = mine_inner(state.clone(), input)
            .await
            .map_err(|error| eyre!(error.message))?;
        assert!(cached.cache_hit);
        assert_eq!(cached.salt, response.salt);
        assert_eq!(cached.address, response.address);
        assert_eq!(cached.score, response.score);
        assert_eq!(cached.runtime_ms, response.runtime_ms);
        assert_eq!(stored_job(&state.cache, &key)?.status, JobStatus::Succeeded);
        assert!(claim_next_job(&state.cache)?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn disconnect_during_enqueue_still_notifies_the_worker() -> Result<()> {
        use std::{future::Future, task::Poll};

        let state = state();
        let key = serde_json::to_string(&normalize_request(request())?)?;
        let enqueued = state.jobs_changed.notified();
        let write_lock = state.cache.begin_write()?;
        {
            let mut client = Box::pin(mine_inner(state.clone(), request()));
            // Start the blocking database task, then disconnect before it can commit.
            std::future::poll_fn(|context| {
                assert!(client.as_mut().poll(context).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        drop(write_lock);
        enqueued.await;
        assert_eq!(claim_next_job(&state.cache)?.unwrap().0, key);
        Ok(())
    }

    #[test]
    fn committed_jobs_and_results_survive_process_exit_without_database_drop() -> Result<()> {
        const CHILD_PATH: &str = "SALTY_REDB_PERSISTENCE_TEST_PATH";
        let request = normalize_request(request())?;
        let key = serde_json::to_string(&request)?;
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let cache = open_cache(std::path::Path::new(&path))?;
            enqueue_job(&cache, &key, &request)?;
            assert_eq!(claim_next_job(&cache)?.unwrap().0, key);
            let mut other = request.clone();
            other.zeros = 7;
            let complete = serde_json::to_string(&other)?;
            enqueue_job(&cache, &complete, &other)?;
            assert_eq!(claim_next_job(&cache)?.unwrap().0, complete);
            complete_job(
                &cache,
                &complete,
                &mining_response(crate::miner::MiningRun {
                    outcome: None,
                    runtime: std::time::Duration::from_millis(20_123),
                }),
            )?;
            other.zeros = 8;
            let failed = serde_json::to_string(&other)?;
            enqueue_job(&cache, &failed, &other)?;
            assert_eq!(claim_next_job(&cache)?.unwrap().0, failed);
            mark_job_failed(&cache, &failed, "device failed".to_owned())?;
            other.zeros = 9;
            enqueue_job(&cache, &serde_json::to_string(&other)?, &other)?;
            // Exit without running Database::drop: commits must already be durable.
            std::process::exit(0);
        }
        let path = std::env::temp_dir().join(format!(
            "salty-persistence-{}-{}.redb",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        let status = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "server::tests::committed_jobs_and_results_survive_process_exit_without_database_drop",
                "--nocapture",
            ])
            .env(CHILD_PATH, &path)
            .status()?;
        assert!(status.success());
        {
            let cache = open_cache(&path)?;
            assert_eq!(claim_next_job(&cache)?.unwrap().0, key);
            let mut other = request;
            other.zeros = 7;
            let complete = serde_json::to_string(&other)?;
            assert_eq!(stored_job(&cache, &complete)?.status, JobStatus::Succeeded);
            assert_eq!(
                get_cached_response(&cache, &complete)?.unwrap().runtime_ms,
                20_123
            );
            enqueue_job(&cache, &complete, &other)?;
            other.zeros = 8;
            assert_eq!(
                get_job_error(&cache, &serde_json::to_string(&other)?)?.as_deref(),
                Some("device failed")
            );
            other.zeros = 9;
            assert_eq!(
                claim_next_job(&cache)?.unwrap().0,
                serde_json::to_string(&other)?
            );
            assert!(claim_next_job(&cache)?.is_none());
        }
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn completing_a_missing_job_does_not_save_an_orphan_result() -> Result<()> {
        let state = state();
        let key = serde_json::to_string(&normalize_request(request())?)?;
        let response = mining_response(crate::miner::MiningRun {
            outcome: None,
            runtime: std::time::Duration::ZERO,
        });
        assert!(complete_job(&state.cache, &key, &response).is_err());
        assert!(get_cached_response(&state.cache, &key)?.is_none());
        assert!(claim_next_job(&state.cache)?.is_none());
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
        enqueue_job(&state.cache, &key, &request)?;
        assert!(run_next_job(&state).await?);
        let response = get_cached_response(&state.cache, &key)?.unwrap();
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
        assert!(claim_next_job(&state.cache)?.is_none());
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
        enqueue_job(&state.cache, &key, &request)?;
        claim_next_job(&state.cache)?.unwrap();
        init_cache(&state.cache)?;
        let (claimed_key, claimed) = claim_next_job(&state.cache)?.unwrap();
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
    fn cache_separates_both_limits() -> Result<()> {
        let state = state();
        let request = normalize_request(request())?;
        let key = serde_json::to_string(&request)?;
        let response = mining_response(crate::miner::MiningRun {
            outcome: None,
            runtime: std::time::Duration::from_millis(20_123),
        });
        enqueue_job(&state.cache, &key, &request)?;
        claim_next_job(&state.cache)?.unwrap();
        complete_job(&state.cache, &key, &response)?;
        assert_eq!(
            get_cached_response(&state.cache, &key)?.unwrap().runtime_ms,
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
            assert!(get_cached_response(&state.cache, &serde_json::to_string(&other)?)?.is_none());
        }
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
