use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use url::Url;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use super::{Proof, Start};
use crate::{
    accounts::{
        AuthError,
        routes::{AuthResponse, Client, ErrorBody, ErrorResponse, error},
    },
    http::AppState,
    sessions::{TokenPair, routes::Authorized},
};

#[derive(OpenApi)]
#[openapi(
    paths(start, callback, exchange, link),
    components(schemas(Exchange, Link, Proof))
)]
pub(crate) struct GoogleDoc;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/google/start", get(start))
        .route("/v1/auth/google/callback", get(callback))
        .route("/v1/auth/google/exchange", post(exchange))
        .route("/v1/auth/google/link", post(link))
        .layer(DefaultBodyLimit::max(8 * 1024))
}

#[derive(Deserialize)]
struct Callback {
    state: String,
    code: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct Exchange {
    #[schema(min_length = 64, max_length = 64)]
    code: String,
    #[schema(min_length = 43, max_length = 128)]
    code_verifier: String,
    installation_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct Link {
    #[schema(min_length = 64, max_length = 64)]
    code: String,
    #[schema(min_length = 43, max_length = 128)]
    code_verifier: String,
    #[schema(min_length = 15, max_length = 128)]
    password: String,
}

fn redirect(url: &Url) -> Response {
    let mut response = Redirect::to(url.as_str()).into_response();
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

#[utoipa::path(
    get, path = "/v1/auth/google/start", params(Start),
    responses((status = 303, description = "Google authorization URL"),
        (status = 400, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn start(
    State(state): State<AppState>,
    client: Client,
    query: Result<Query<Start>, QueryRejection>,
) -> Result<Response, ErrorResponse> {
    let google = state
        .google
        .as_ref()
        .ok_or_else(|| error(AuthError::Unavailable, client.request_id.clone()))?;
    let Query(request) =
        query.map_err(|_| error(AuthError::InvalidOAuth, client.request_id.clone()))?;
    super::start(&state.pool, &state.accounts, google, client.ip, request)
        .await
        .map(|url| redirect(&url))
        .map_err(|err| error(err, client.request_id))
}

#[utoipa::path(
    get, path = "/v1/auth/google/callback",
    params(("state" = String, Query), ("code" = Option<String>, Query), ("error" = Option<String>, Query)),
    responses((status = 303, description = "One-time code or error to the desktop loopback listener"),
        (status = 400, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn callback(
    State(state): State<AppState>,
    client: Client,
    query: Result<Query<Callback>, QueryRejection>,
) -> Result<Response, ErrorResponse> {
    let google = state
        .google
        .as_ref()
        .ok_or_else(|| error(AuthError::Unavailable, client.request_id.clone()))?;
    let Query(request) =
        query.map_err(|_| error(AuthError::InvalidOAuth, client.request_id.clone()))?;
    let code = if request.error.is_none() {
        request.code.as_deref()
    } else {
        None
    };
    super::callback(&state.pool, google, &request.state, code)
        .await
        .map(|url| redirect(&url))
        .map_err(|err| error(err, client.request_id))
}

#[utoipa::path(
    post, path = "/v1/auth/google/exchange", request_body = Exchange,
    responses((status = 200, description = "A new Moby session and token pair", body = TokenPair),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn exchange(
    State(state): State<AppState>,
    client: Client,
    input: Result<Json<Exchange>, JsonRejection>,
) -> Result<Json<TokenPair>, ErrorResponse> {
    if state.google.is_none() {
        return Err(error(AuthError::Unavailable, client.request_id));
    }
    let input = client.read(input)?;
    let proof = Proof {
        code: input.code,
        code_verifier: input.code_verifier,
    };
    super::exchange(
        &state.pool,
        &state.accounts,
        &state.access_keys,
        client.ip,
        proof,
        input.installation_id,
    )
    .await
    .map(Json)
    .map_err(|err| error(err, client.request_id))
}

#[utoipa::path(
    post, path = "/v1/auth/google/link", request_body = Link, security(("bearer" = [])),
    responses((status = 204, description = "Google linked after password confirmation"),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn link(
    State(state): State<AppState>,
    auth: Authorized,
    client: Client,
    input: Result<Json<Link>, JsonRejection>,
) -> AuthResponse {
    if state.google.is_none() {
        return Err(error(AuthError::Unavailable, client.request_id));
    }
    let input = client.read(input)?;
    let proof = Proof {
        code: input.code,
        code_verifier: input.code_verifier,
    };
    let result = super::link(
        &state.pool,
        &state.accounts,
        auth.identity,
        client.ip,
        proof,
        input.password,
    )
    .await;
    client.reply(StatusCode::NO_CONTENT, result)
}
