use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderValue, StatusCode, header},
    routing::{get, post},
};
use serde::{Deserialize, Deserializer};
use utoipa::{OpenApi, ToSchema};

use super::{
    AccountExport, ConsentKind, ConsentRecord, ConsentVersions, Consents, ExportAccount,
    ExportIdentity, ExportInstallation, ExportSession, Profile,
};
use crate::{
    accounts::{
        AuthError,
        routes::{AuthResponse, Client, ErrorBody, ErrorResponse, error},
    },
    google,
    http::AppState,
    sessions::routes::Authorized,
};

#[derive(OpenApi)]
#[openapi(
    paths(me, update, export, delete, list_consents, set_consent),
    components(schemas(
        Profile,
        UpdateProfile,
        DeleteAccount,
        PasswordDelete,
        AccountExport,
        ExportAccount,
        ExportIdentity,
        ExportInstallation,
        ExportSession,
        ConsentKind,
        ConsentRecord,
        ConsentVersions,
        Consents,
        SetConsent
    ))
)]
pub(crate) struct ProfileDoc;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/me", get(me).patch(update).delete(delete))
        .route("/v1/me/export", post(export))
        .route("/v1/consents", get(list_consents).post(set_consent))
        .layer(DefaultBodyLimit::max(8 * 1024))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateProfile {
    #[serde(deserialize_with = "required_nullable_name")]
    #[schema(required = true, min_length = 1, max_length = 100)]
    display_name: Option<String>,
}

fn required_nullable_name<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Deserialize, ToSchema)]
#[serde(untagged)]
enum DeleteAccount {
    Password(PasswordDelete),
    Google(google::Proof),
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct PasswordDelete {
    #[schema(min_length = 15, max_length = 128)]
    password: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SetConsent {
    #[serde(rename = "type")]
    kind: ConsentKind,
    #[schema(min_length = 1, max_length = 64)]
    version: String,
    accepted: bool,
}

#[utoipa::path(
    get, path = "/v1/me", security(("bearer" = [])),
    responses((status = 200, description = "Signed-in profile", body = Profile),
        (status = 401, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn me(
    State(state): State<AppState>,
    auth: Authorized,
) -> Result<Json<Profile>, ErrorResponse> {
    super::get(&state.pool, auth.identity)
        .await
        .map(Json)
        .map_err(|err| error(err, auth.request_id))
}

#[utoipa::path(
    patch, path = "/v1/me", request_body = UpdateProfile, security(("bearer" = [])),
    responses((status = 200, description = "Updated profile", body = Profile),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn update(
    State(state): State<AppState>,
    auth: Authorized,
    client: Client,
    input: Result<Json<UpdateProfile>, JsonRejection>,
) -> Result<Json<Profile>, ErrorResponse> {
    let input = client.read(input)?;
    super::update(&state.pool, auth.identity, input.display_name.as_deref())
        .await
        .map(Json)
        .map_err(|err| error(err, auth.request_id))
}

type ExportResponse = ([(header::HeaderName, HeaderValue); 1], Json<AccountExport>);

#[utoipa::path(
    post, path = "/v1/me/export", security(("bearer" = [])),
    responses((status = 200, description = "Account data export", body = AccountExport,
        headers(("Content-Disposition" = String, description = "JSON attachment"))),
        (status = 401, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn export(
    State(state): State<AppState>,
    auth: Authorized,
) -> Result<ExportResponse, ErrorResponse> {
    let result = super::export(&state.pool, auth.identity.user_id)
        .await
        .map_err(|err| error(err, auth.request_id))?;
    Ok((
        [(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"moby-account-export.json\""),
        )],
        Json(result),
    ))
}

#[utoipa::path(
    delete, path = "/v1/me", request_body = DeleteAccount, security(("bearer" = [])),
    responses((status = 204, description = "Account and its stored records deleted"),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn delete(
    State(state): State<AppState>,
    auth: Authorized,
    client: Client,
    input: Result<Json<DeleteAccount>, JsonRejection>,
) -> AuthResponse {
    let input = client.read(input)?;
    let result = match input {
        DeleteAccount::Password(input) => {
            super::delete(
                &state.pool,
                &state.accounts,
                auth.identity,
                client.ip,
                input.password,
            )
            .await
        }
        DeleteAccount::Google(proof) if state.google.is_some() => {
            google::delete_account(&state.pool, auth.identity, client.ip, proof).await
        }
        DeleteAccount::Google(_) => Err(AuthError::Unavailable),
    };
    client.reply(StatusCode::NO_CONTENT, result)
}

#[utoipa::path(
    get, path = "/v1/consents", security(("bearer" = [])),
    responses((status = 200, description = "Current policy versions and account consent records", body = Consents),
        (status = 401, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn list_consents(
    State(state): State<AppState>,
    auth: Authorized,
) -> Result<Json<Consents>, ErrorResponse> {
    super::list_consents(&state.pool, state.accounts.config(), auth.identity.user_id)
        .await
        .map(Json)
        .map_err(|err| error(err, auth.request_id))
}

#[utoipa::path(
    post, path = "/v1/consents", request_body = SetConsent, security(("bearer" = [])),
    responses((status = 204, description = "Consent granted or withdrawn"),
        (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 503, body = ErrorBody))
)]
async fn set_consent(
    State(state): State<AppState>,
    auth: Authorized,
    client: Client,
    input: Result<Json<SetConsent>, JsonRejection>,
) -> AuthResponse {
    let input = client.read(input)?;
    let result = super::set_consent(
        &state.pool,
        state.accounts.config(),
        auth.identity.user_id,
        input.kind,
        &input.version,
        input.accepted,
    )
    .await;
    client.reply(StatusCode::NO_CONTENT, result)
}
