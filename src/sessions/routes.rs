use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, FromRequestParts, Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use utoipa::{
    Modify, OpenApi, ToSchema,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};
use uuid::Uuid;

use super::{Identity, Installation, TokenPair};
use crate::{
    accounts::{
        AuthError,
        routes::{AuthResponse, Client, ErrorBody, ErrorResponse, error},
    },
    http::{AppState, RequestId},
};

#[derive(OpenApi)]
#[openapi(
    paths(login, refresh, logout, installations, add_installation, revoke_installation),
    components(schemas(Login, Refresh, NewInstallation, TokenPair, Installation)),
    modifiers(&BearerScheme)
)]
pub(crate) struct SessionDoc;

struct BearerScheme;

impl Modify for BearerScheme {
    fn modify(&self, doc: &mut utoipa::openapi::OpenApi) {
        doc.components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "bearer",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
    }
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/login", post(login))
        .route("/v1/auth/refresh", post(refresh))
        .route("/v1/auth/logout", post(logout))
        .route(
            "/v1/installations",
            get(installations).post(add_installation),
        )
        .route("/v1/installations/{id}", delete(revoke_installation))
        .layer(DefaultBodyLimit::max(8 * 1024))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct Login {
    #[schema(format = Email, max_length = 254)]
    email: String,
    #[schema(min_length = 15, max_length = 128)]
    password: String,
    installation_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct Refresh {
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-fA-F]{64}$")]
    refresh_token: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct NewInstallation {
    #[schema(min_length = 1, max_length = 100)]
    name: String,
    #[schema(min_length = 1, max_length = 64)]
    app_version: Option<String>,
}

pub(crate) struct Authorized {
    pub(crate) identity: Identity,
    pub(crate) request_id: String,
}

impl FromRequestParts<AppState> for Authorized {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let request_id = parts
            .extensions
            .get::<RequestId>()
            .map_or_else(|| Uuid::new_v4().to_string(), |id| id.0.clone());
        let mut headers = parts.headers.get_all(header::AUTHORIZATION).iter();
        let token = headers
            .next()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .filter(|_| headers.next().is_none())
            .map(|(_, token)| token);
        let result = match token {
            Some(token) => super::authorize(&state.pool, &state.access_keys, token).await,
            None => Err(AuthError::Unauthorized),
        };
        match result {
            Ok(identity) => Ok(Self {
                identity,
                request_id,
            }),
            Err(err) => {
                let mut response = error(err, request_id).into_response();
                if response.status() == StatusCode::UNAUTHORIZED {
                    response.headers_mut().insert(
                        header::WWW_AUTHENTICATE,
                        header::HeaderValue::from_static("Bearer"),
                    );
                }
                Err(response)
            }
        }
    }
}

#[utoipa::path(
    post, path = "/v1/auth/login", request_body = Login,
    responses((status = 200, description = "A new session and token pair", body = TokenPair),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn login(
    State(state): State<AppState>,
    client: Client,
    input: Result<Json<Login>, JsonRejection>,
) -> Result<Json<TokenPair>, ErrorResponse> {
    let data = client.read(input)?;
    super::login(
        &state.pool,
        &state.accounts,
        &state.access_keys,
        client.ip,
        data.email,
        data.password,
        data.installation_id,
    )
    .await
    .map(Json)
    .map_err(|err| error(err, client.request_id))
}

#[utoipa::path(
    post, path = "/v1/auth/refresh", request_body = Refresh,
    responses((status = 200, description = "Rotated token pair", body = TokenPair),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn refresh(
    State(state): State<AppState>,
    client: Client,
    input: Result<Json<Refresh>, JsonRejection>,
) -> Result<Json<TokenPair>, ErrorResponse> {
    let data = client.read(input)?;
    super::refresh(
        &state.pool,
        &state.access_keys,
        client.ip,
        &data.refresh_token,
    )
    .await
    .map(Json)
    .map_err(|err| error(err, client.request_id))
}

#[utoipa::path(
    post, path = "/v1/auth/logout", request_body = Refresh,
    responses((status = 204, description = "The session was revoked or already absent"),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn logout(
    State(state): State<AppState>,
    client: Client,
    input: Result<Json<Refresh>, JsonRejection>,
) -> AuthResponse {
    let data = client.read(input)?;
    let result = super::logout(&state.pool, client.ip, &data.refresh_token).await;
    client.reply(StatusCode::NO_CONTENT, result)
}

#[utoipa::path(
    get, path = "/v1/installations", security(("bearer" = [])),
    responses((status = 200, description = "Installations owned by the signed-in account", body = [Installation]),
        (status = 401, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn installations(
    State(state): State<AppState>,
    auth: Authorized,
) -> Result<Json<Vec<Installation>>, ErrorResponse> {
    super::list_installations(&state.pool, auth.identity.user_id)
        .await
        .map(Json)
        .map_err(|err| error(err, auth.request_id))
}

#[utoipa::path(
    post, path = "/v1/installations", request_body = NewInstallation, security(("bearer" = [])),
    responses((status = 201, description = "Windows installation created", body = Installation),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn add_installation(
    State(state): State<AppState>,
    auth: Authorized,
    client: Client,
    input: Result<Json<NewInstallation>, JsonRejection>,
) -> Result<(StatusCode, Json<Installation>), ErrorResponse> {
    let data = client.read(input)?;
    super::add_installation(
        &state.pool,
        auth.identity.user_id,
        &data.name,
        data.app_version.as_deref(),
    )
    .await
    .map(|installation| (StatusCode::CREATED, Json(installation)))
    .map_err(|err| error(err, auth.request_id))
}

#[utoipa::path(
    delete, path = "/v1/installations/{id}", params(("id" = Uuid, Path)), security(("bearer" = [])),
    responses((status = 204, description = "Installation and its sessions revoked"),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn revoke_installation(
    State(state): State<AppState>,
    auth: Authorized,
    path: Result<Path<Uuid>, PathRejection>,
) -> AuthResponse {
    let Path(id) =
        path.map_err(|_| error(AuthError::InvalidInstallation, auth.request_id.clone()))?;
    super::revoke_installation(&state.pool, auth.identity.user_id, id)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|err| error(err, auth.request_id))
}
