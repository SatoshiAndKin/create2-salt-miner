use std::{
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
use eyre::{Context, Result, eyre};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
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
    miner_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Serialize, ToSchema)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
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

#[derive(Debug, Serialize, ToSchema)]
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

    let state = ServerState {
        cache: Arc::new(Mutex::new(connection)),
        miner_lock: Arc::new(tokio::sync::Mutex::new(())),
    };

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
    let config = normalized
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

    let _permit = state.miner_lock.lock().await;

    if let Some(mut response) =
        get_cached_response(&state, &request_key).map_err(ServerError::internal)?
    {
        response.cache_hit = true;
        return Ok(response);
    }

    let stop = normalized.stop_mode();
    let outcome = tokio::task::spawn_blocking(move || mine_once(config, stop))
        .await
        .map_err(|error| ServerError::internal(eyre!("mining task failed to join: {error}")))?
        .map_err(ServerError::internal)?;

    let response = match outcome {
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
            runtime_ms: normalized
                .max_runtime_secs
                .map_or(0, |secs| u128::from(secs) * u128::from(1_000_u16)),
        },
    };

    if response.found {
        insert_cached_response(&state, &request_key, &normalized, &response)
            .map_err(ServerError::internal)?;
    }

    Ok(response)
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
