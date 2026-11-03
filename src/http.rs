use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use moby_protocol::ApiError;
use serde::Serialize;
use sqlx::PgPool;
use tracing::{Instrument, info, info_span};
use utoipa::{
    OpenApi, ToSchema,
    openapi::{OpenApi as OpenApiDocument, info::LicenseBuilder},
};
use uuid::Uuid;

use crate::{
    AccessKeys, Demo, Google, MIGRATOR, UsageConfig,
    accounts::{Accounts, routes},
    agent::{self, circuit::Circuit},
    config::AccountConfig,
    demo, google, profile, sessions,
    usage::UsageError,
    voice,
};

pub(crate) mod schema;

pub(crate) type ApiFailure = (StatusCode, Json<ApiError>);

pub(crate) fn api_error(code: &str, message: &str, request_id: String) -> ApiError {
    ApiError {
        code: code.into(),
        message: message.into(),
        request_id,
    }
}

pub(crate) fn usage_failure(error: UsageError, request_id: String) -> ApiFailure {
    let (status, code, message) = match error {
        UsageError::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A verified session is required",
        ),
        UsageError::VoiceConsentRequired => (
            StatusCode::FORBIDDEN,
            "voice_consent_required",
            "Accept the current cloud_voice policy",
        ),
        UsageError::QuotaExceeded => (
            StatusCode::TOO_MANY_REQUESTS,
            "quota_exceeded",
            "The request exceeds the remaining daily quota",
        ),
        UsageError::BudgetExhausted => (
            StatusCode::SERVICE_UNAVAILABLE,
            "service_budget_exhausted",
            "The service budget is exhausted",
        ),
        UsageError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "usage_unavailable",
            "Usage could not be checked",
        ),
    };
    (status, Json(api_error(code, message, request_id)))
}

#[derive(Clone)]
pub(crate) struct RequestId(pub String);

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) accounts: Accounts,
    pub(crate) access_keys: AccessKeys,
    pub(crate) google: Option<Google>,
    pub(crate) usage: UsageConfig,
    pub(crate) agent_circuit: Circuit,
    pub(crate) demo: Option<Demo>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ProbeStatus {
    Ok,
    Ready,
    NotReady,
}

#[derive(Serialize, ToSchema)]
struct ProbeBody {
    status: ProbeStatus,
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Moby API"),
    paths(health, ready, openapi_endpoint),
    components(schemas(ProbeBody, ProbeStatus))
)]
struct ApiDoc;

pub fn openapi() -> OpenApiDocument {
    let mut document = ApiDoc::openapi();
    document.info.license = Some(
        LicenseBuilder::new()
            .name("GNU General Public License v3.0 only")
            .identifier(Some(env!("CARGO_PKG_LICENSE")))
            .build(),
    );
    document.merge(routes::AuthDoc::openapi());
    document.merge(sessions::routes::SessionDoc::openapi());
    document.merge(profile::routes::ProfileDoc::openapi());
    document.merge(google::routes::GoogleDoc::openapi());
    agent::routes::add_openapi(&mut document);
    voice::add_openapi(&mut document);
    document.merge(demo::DemoDoc::openapi());
    document
}

pub fn router(
    pool: PgPool,
    config: AccountConfig,
    access_keys: AccessKeys,
    google: Option<Google>,
) -> Router {
    router_with_usage(pool, config, access_keys, google, UsageConfig::default())
}

pub fn router_with_usage(
    pool: PgPool,
    config: AccountConfig,
    access_keys: AccessKeys,
    google: Option<Google>,
    usage: UsageConfig,
) -> Router {
    router_with_demo(pool, config, access_keys, google, usage, None)
}

pub fn router_with_demo(
    pool: PgPool,
    config: AccountConfig,
    access_keys: AccessKeys,
    google: Option<Google>,
    usage: UsageConfig,
    demo: Option<Demo>,
) -> Router {
    let accounts = Accounts::new(pool.clone(), config);
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/openapi.json", get(openapi_endpoint))
        .merge(routes::router())
        .merge(sessions::routes::router())
        .merge(profile::routes::router())
        .merge(google::routes::router())
        .merge(agent::routes::router())
        .merge(voice::router())
        .merge(demo::router())
        .with_state(AppState {
            pool,
            accounts,
            access_keys,
            google,
            usage,
            agent_circuit: Circuit::default(),
            demo,
        })
        .layer(middleware::from_fn(trace_request))
}

#[utoipa::path(
    get, path = "/health",
    responses((status = 200, description = "API process is running", body = ProbeBody))
)]
async fn health() -> Json<ProbeBody> {
    Json(ProbeBody {
        status: ProbeStatus::Ok,
    })
}

#[utoipa::path(
    get, path = "/ready",
    responses(
        (status = 200, description = "PostgreSQL and the required schema are ready", body = ProbeBody),
        (status = 503, description = "PostgreSQL or the required schema is unavailable", body = ProbeBody)
    )
)]
async fn ready(State(state): State<AppState>) -> (StatusCode, Json<ProbeBody>) {
    let version = MIGRATOR
        .iter()
        .last()
        .expect("account migration is embedded")
        .version;
    let probe = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = $1 AND success)",
    )
    .bind(version)
    .fetch_one(&state.pool);
    let (code, status) = match tokio::time::timeout(Duration::from_secs(2), probe).await {
        Ok(Ok(true)) => (StatusCode::OK, ProbeStatus::Ready),
        _ => (StatusCode::SERVICE_UNAVAILABLE, ProbeStatus::NotReady),
    };

    (code, Json(ProbeBody { status }))
}

#[utoipa::path(
    get, path = "/openapi.json",
    responses((status = 200, description = "OpenAPI document", body = Object))
)]
async fn openapi_endpoint() -> Json<OpenApiDocument> {
    Json(openapi())
}

async fn trace_request(mut request: Request, next: Next) -> Response {
    let request_id = Uuid::new_v4().to_string();
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str);
    let span = info_span!("http_request", %request_id, route);
    let start = Instant::now();

    async move {
        let mut response = next.run(request).await;
        response.headers_mut().insert(
            "x-request-id",
            HeaderValue::from_str(&request_id).expect("UUID is a valid HTTP header value"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        info!(
            status = response.status().as_u16(),
            elapsed_ms = start.elapsed().as_millis() as u64,
            "request finished"
        );
        response
    }
    .instrument(span)
    .await
}
