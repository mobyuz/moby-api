use std::net::{IpAddr, SocketAddr};

use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, State, rejection::JsonRejection},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::Html,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use super::{AuthError, RESET_PATH, VERIFY_PATH};
use crate::http::{AppState, RequestId};

#[derive(OpenApi)]
#[openapi(
    paths(register, verify, forgot, reset, verify_form, reset_form),
    components(schemas(Register, Verify, Forgot, Reset, ErrorBody))
)]
pub(crate) struct AuthDoc;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/register", post(register))
        .route(VERIFY_PATH, get(verify_form).post(verify))
        .route("/v1/auth/password/forgot", post(forgot))
        .route(RESET_PATH, get(reset_form).post(reset))
        .layer(DefaultBodyLimit::max(8 * 1024))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct Register {
    #[schema(format = Email, max_length = 254)]
    email: String,
    #[schema(min_length = 15, max_length = 128)]
    password: String,
    #[schema(min_length = 1, max_length = 64)]
    terms_version: String,
    #[schema(min_length = 1, max_length = 64)]
    privacy_policy_version: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct Verify {
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-fA-F]{64}$")]
    token: String,
    #[schema(min_length = 15, max_length = 128)]
    password: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct Forgot {
    #[schema(format = Email, max_length = 254)]
    email: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct Reset {
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-fA-F]{64}$")]
    token: String,
    #[schema(min_length = 15, max_length = 128)]
    new_password: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ErrorBody {
    code: &'static str,
    message: &'static str,
    request_id: String,
}

pub(crate) type ErrorResponse = (StatusCode, Json<ErrorBody>);
pub(crate) type AuthResponse = Result<StatusCode, ErrorResponse>;

pub(crate) struct Client {
    pub(crate) ip: IpAddr,
    pub(crate) request_id: String,
}

impl FromRequestParts<AppState> for Client {
    type Rejection = ErrorResponse;

    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, Self::Rejection> {
        let id = parts
            .extensions
            .get::<RequestId>()
            .map_or_else(|| Uuid::new_v4().to_string(), |id| id.0.clone());
        let Some(ConnectInfo(peer)) = parts.extensions.get::<ConnectInfo<SocketAddr>>() else {
            return Err(error(AuthError::Unavailable, id));
        };
        Ok(Self {
            ip: peer.ip().to_canonical(),
            request_id: id,
        })
    }
}

impl Client {
    pub(crate) fn read<T>(&self, body: Result<Json<T>, JsonRejection>) -> Result<T, ErrorResponse> {
        body.map(|Json(data)| data).map_err(|err| {
            let (status, code, message) = if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "payload_too_large",
                    "Request body exceeds the size limit",
                )
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "Expected a valid JSON request",
                )
            };
            (
                status,
                Json(ErrorBody {
                    code,
                    message,
                    request_id: self.request_id.clone(),
                }),
            )
        })
    }

    pub(crate) fn reply(self, status: StatusCode, result: Result<(), AuthError>) -> AuthResponse {
        result
            .map(|()| status)
            .map_err(|err| error(err, self.request_id))
    }
}

pub(crate) fn error(error: AuthError, request_id: String) -> ErrorResponse {
    use AuthError::*;
    let (status, code, message) = match error {
        InvalidEmail => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Enter a valid ASCII email address",
        ),
        InvalidPassword => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Use a password with 15–128 characters",
        ),
        InvalidConsent => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Accept the current terms and privacy policy versions",
        ),
        InvalidToken => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Link or credentials are invalid or expired",
        ),
        InvalidInstallation => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Installation details are invalid",
        ),
        InvalidProfileName => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Display name must contain 1–100 characters or be null",
        ),
        InvalidConsentVersion => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Use a valid policy version. Grants require the current version",
        ),
        InvalidOAuth => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Google sign-in details are invalid or expired",
        ),
        AccountLinkRequired => (
            StatusCode::CONFLICT,
            "account_link_required",
            "Sign in to your Moby account to link Google",
        ),
        IdentityConflict => (
            StatusCode::CONFLICT,
            "identity_conflict",
            "Google identity could not be linked to this account",
        ),
        ProviderUnavailable => (
            StatusCode::BAD_GATEWAY,
            "provider_unavailable",
            "Google sign-in is temporarily unavailable",
        ),
        Unauthorized => (
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Credentials are invalid or expired",
        ),
        NotFound => (StatusCode::NOT_FOUND, "not_found", "Resource was not found"),
        RateLimited => (
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many requests. Try again later",
        ),
        Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Account service is temporarily unavailable",
        ),
    };
    (
        status,
        Json(ErrorBody {
            code,
            message,
            request_id,
        }),
    )
}

#[utoipa::path(
    post, path = "/v1/auth/register", request_body = Register,
    responses((status = 202, description = "Registration request accepted"),
        (status = 400, body = ErrorBody), (status = 413, body = ErrorBody),
        (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn register(
    State(state): State<AppState>,
    client: Client,
    input: Result<Json<Register>, JsonRejection>,
) -> AuthResponse {
    let data = client.read(input)?;
    let result = state
        .accounts
        .register(
            &state.pool,
            client.ip,
            data.email,
            data.password,
            data.terms_version,
            data.privacy_policy_version,
        )
        .await;
    client.reply(StatusCode::ACCEPTED, result)
}

#[utoipa::path(
    post, path = "/v1/auth/verify-email", request_body = Verify,
    responses((status = 204, description = "Email verified and account activated"),
        (status = 400, body = ErrorBody), (status = 413, body = ErrorBody),
        (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn verify(
    State(state): State<AppState>,
    client: Client,
    input: Result<Json<Verify>, JsonRejection>,
) -> AuthResponse {
    let data = client.read(input)?;
    let result = state
        .accounts
        .verify_email(&state.pool, client.ip, data.token, data.password)
        .await;
    client.reply(StatusCode::NO_CONTENT, result)
}

#[utoipa::path(
    post, path = "/v1/auth/password/forgot", request_body = Forgot,
    responses((status = 202, description = "Request accepted regardless of account existence"),
        (status = 400, body = ErrorBody), (status = 413, body = ErrorBody),
        (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn forgot(
    State(state): State<AppState>,
    client: Client,
    input: Result<Json<Forgot>, JsonRejection>,
) -> AuthResponse {
    let data = client.read(input)?;
    let result = state
        .accounts
        .forgot(&state.pool, client.ip, data.email)
        .await;
    client.reply(StatusCode::ACCEPTED, result)
}

#[utoipa::path(
    post, path = "/v1/auth/password/reset", request_body = Reset,
    responses((status = 204, description = "Password reset"),
        (status = 400, body = ErrorBody), (status = 413, body = ErrorBody),
        (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn reset(
    State(state): State<AppState>,
    client: Client,
    input: Result<Json<Reset>, JsonRejection>,
) -> AuthResponse {
    let data = client.read(input)?;
    let result = state
        .accounts
        .reset(&state.pool, client.ip, data.token, data.new_password)
        .await;
    client.reply(StatusCode::NO_CONTENT, result)
}

#[utoipa::path(
    get, path = "/v1/auth/verify-email",
    responses((status = 200, description = "Email link form", body = String, content_type = "text/html"))
)]
async fn verify_form() -> ([(header::HeaderName, HeaderValue); 2], Html<String>) {
    form(
        "Verify your Moby email",
        "Registration password",
        "password",
        "current-password",
    )
}

#[utoipa::path(
    get, path = "/v1/auth/password/reset",
    responses((status = 200, description = "Password reset link form", body = String, content_type = "text/html"))
)]
async fn reset_form() -> ([(header::HeaderName, HeaderValue); 2], Html<String>) {
    form(
        "Reset your Moby password",
        "New password",
        "new_password",
        "new-password",
    )
}

fn form(
    title: &str,
    label: &str,
    field: &str,
    autocomplete: &str,
) -> ([(header::HeaderName, HeaderValue); 2], Html<String>) {
    let nonce = Uuid::new_v4().simple().to_string();
    let policy = format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; connect-src 'self'; frame-ancestors 'none'; form-action 'none'; base-uri 'none'"
    );
    let headers = [
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(&policy).expect("valid CSP"),
        ),
        (
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ),
    ];
    let html = include_str!("form.html")
        .replace("{{TITLE}}", title)
        .replace("{{LABEL}}", label)
        .replace("{{FIELD}}", field)
        .replace("{{AUTOCOMPLETE}}", autocomplete)
        .replace("{{NONCE}}", &nonce);
    (headers, Html(html))
}
