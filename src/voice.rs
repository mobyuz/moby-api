use std::{future::Future, time::Duration};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, Query, State,
        rejection::{BytesRejection, JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use moby_protocol::{Language, Script};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use utoipa::openapi::{
    Content, HttpMethod, KnownFormat, Object, OpenApi, Ref, Required, ResponseBuilder,
    SchemaFormat, SecurityRequirement, Type,
    path::{OperationBuilder, ParameterBuilder, ParameterIn, PathItem},
    request_body::RequestBodyBuilder,
};
use uuid::Uuid;

use crate::{
    http::{
        ApiFailure, AppState, api_error,
        schema::{add_schema, json_response},
        usage_failure,
    },
    sessions::routes::Authorized,
    usage::{self, Charge, Work},
};

mod provider;
#[cfg(test)]
mod tests;
mod wave;

use provider::{MockSpeech, ProviderResult, SpeechProvider};

const STT_PATH: &str = "/v1/voice/stt";
const TTS_PATH: &str = "/v1/voice/tts";
const AUDIO_TYPE: &str = "audio/wav";
const MAX_TTS_CHARS: usize = 500;
const MAX_TTS_BODY: usize = 8 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 8 * 1024;
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy)]
struct Locale {
    language: Language,
    script: Script,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocaleHints {
    language: Option<Language>,
    script: Option<Script>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TtsRequest {
    #[schemars(length(min = 1, max = MAX_TTS_CHARS))]
    text: String,
    language: Option<Language>,
    script: Option<Script>,
}

#[derive(Serialize, JsonSchema)]
struct Transcript {
    text: String,
    language: Language,
    script: Script,
    #[schemars(range(min = 0, max = 1))]
    confidence: f32,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            STT_PATH,
            post(stt).layer(DefaultBodyLimit::max(wave::MAX_WAV_BYTES)),
        )
        .route(
            TTS_PATH,
            post(tts).layer(DefaultBodyLimit::max(MAX_TTS_BODY)),
        )
}

async fn stt(
    State(state): State<AppState>,
    auth: Authorized,
    headers: HeaderMap,
    hints: Result<Query<LocaleHints>, QueryRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Transcript>, ApiFailure> {
    let mut content_types = headers.get_all(header::CONTENT_TYPE).iter();
    let content_type = content_types.next().and_then(|value| value.to_str().ok());
    if content_types.next().is_some()
        || !content_type.is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case(AUDIO_TYPE)
        })
    {
        return Err(failure(
            &auth,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Send an audio/wav request",
        ));
    }
    let Query(hints) = hints.map_err(|_| invalid_locale(&auth))?;
    let locale = locale(hints.language, hints.script).ok_or_else(|| invalid_locale(&auth))?;
    let body =
        body.map_err(|error| body_failure(&auth, error.status(), "Could not read the audio body"))?;
    let pcm = wave::pcm(&body).map_err(|error| match error {
        wave::WaveError::Invalid => failure(
            &auth,
            StatusCode::BAD_REQUEST,
            "invalid_audio",
            "Send a complete mono 16 kHz 16-bit PCM WAV clip",
        ),
        wave::WaveError::TooLarge => failure(
            &auth,
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "Audio or WAV metadata exceeds the limit",
        ),
    })?;
    let units = u32::try_from(pcm.len().div_ceil(wave::BYTE_RATE)).expect("bounded audio duration");
    let version = &state.accounts.config().cloud_voice_version;
    let provider = MockSpeech;
    let result = execute(
        &state,
        &auth,
        Work::Stt(version),
        Charge {
            units,
            cost_microusd: provider.estimate_stt_cost(units),
        },
        || provider.transcribe(pcm, locale),
    )
    .await?;
    if result.text.len() > MAX_TRANSCRIPT_BYTES
        || !(0.0..=1.0).contains(&result.confidence)
        || locale_for_pair(result.language, result.script).is_none()
    {
        return Err(invalid_provider_output(&auth));
    }
    Ok(Json(result))
}

async fn tts(
    State(state): State<AppState>,
    auth: Authorized,
    body: Result<Json<TtsRequest>, JsonRejection>,
) -> Result<Response, ApiFailure> {
    let Json(request) =
        body.map_err(|error| body_failure(&auth, error.status(), "Expected a valid JSON request"))?;
    let locale = locale(request.language, request.script).ok_or_else(|| invalid_locale(&auth))?;
    let length = request.text.chars().take(MAX_TTS_CHARS + 1).count();
    if length > MAX_TTS_CHARS || request.text.trim().is_empty() {
        return Err(failure(
            &auth,
            StatusCode::BAD_REQUEST,
            "invalid_text",
            "Send between 1 and 500 characters of text",
        ));
    }
    let units = u32::try_from(length).expect("bounded text length");
    let version = &state.accounts.config().cloud_voice_version;
    let provider = MockSpeech;
    let audio = execute(
        &state,
        &auth,
        Work::Tts(version),
        Charge {
            units,
            cost_microusd: provider.estimate_tts_cost(units),
        },
        || provider.synthesize(&request.text, locale),
    )
    .await?;
    let wave = wave::encode(audio).map_err(|_| invalid_provider_output(&auth))?;
    Ok(([(header::CONTENT_TYPE, AUDIO_TYPE)], wave).into_response())
}

async fn execute<T, F: Future<Output = ProviderResult<T>>>(
    state: &AppState,
    auth: &Authorized,
    work: Work<'_>,
    estimated: Charge,
    dispatch: impl FnOnce() -> F,
) -> Result<T, ApiFailure> {
    if !state.usage.voice_enabled {
        return Err(failure(
            auth,
            StatusCode::SERVICE_UNAVAILABLE,
            "voice_disabled",
            "Voice is disabled",
        ));
    }
    let id = Uuid::parse_str(&auth.request_id).expect("server-generated request ID");
    usage::reserve(
        &state.pool,
        state.usage,
        auth.identity.user_id,
        id,
        work,
        estimated,
    )
    .await
    .map_err(|error| usage_failure(error, auth.request_id.clone()))?;
    let (result, actual_cost) = match timeout(PROVIDER_TIMEOUT, dispatch()).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            return Err(failure(
                auth,
                StatusCode::BAD_GATEWAY,
                "voice_provider_failed",
                "Voice provider failed",
            ));
        }
        Err(_) => {
            return Err(failure(
                auth,
                StatusCode::GATEWAY_TIMEOUT,
                "voice_timeout",
                "Voice provider timed out",
            ));
        }
    };
    usage::settle(
        &state.pool,
        id,
        Charge {
            cost_microusd: actual_cost,
            ..estimated
        },
    )
    .await
    .map_err(|error| usage_failure(error, auth.request_id.clone()))?;
    Ok(result)
}

fn locale(language: Option<Language>, script: Option<Script>) -> Option<Locale> {
    let language = language.unwrap_or(Language::Uz);
    let script = script.unwrap_or(if language == Language::Ru {
        Script::Cyrl
    } else {
        Script::Latn
    });
    locale_for_pair(language, script)
}

fn locale_for_pair(language: Language, script: Script) -> Option<Locale> {
    match (language, script) {
        (Language::Ru, Script::Latn) | (Language::En, Script::Cyrl) => None,
        _ => Some(Locale { language, script }),
    }
}

fn failure(auth: &Authorized, status: StatusCode, code: &str, message: &str) -> ApiFailure {
    (
        status,
        Json(api_error(code, message, auth.request_id.clone())),
    )
}

fn invalid_locale(auth: &Authorized) -> ApiFailure {
    failure(
        auth,
        StatusCode::BAD_REQUEST,
        "invalid_locale",
        "Supported pairs are uz/latn, uz/cyrl, ru/cyrl and en/latn",
    )
}

fn invalid_provider_output(auth: &Authorized) -> ApiFailure {
    failure(
        auth,
        StatusCode::BAD_GATEWAY,
        "invalid_voice_output",
        "Voice provider returned invalid output",
    )
}

fn body_failure(auth: &Authorized, status: StatusCode, message: &str) -> ApiFailure {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => failure(
            auth,
            status,
            "payload_too_large",
            "Request body exceeds the size limit",
        ),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => failure(
            auth,
            status,
            "unsupported_media_type",
            "Send an application/json request",
        ),
        _ => failure(auth, StatusCode::BAD_REQUEST, "bad_request", message),
    }
}

pub(crate) fn add_openapi(doc: &mut OpenApi) {
    let schemas = &mut doc.components.get_or_insert_with(Default::default).schemas;
    add_schema::<TtsRequest>(schemas);
    add_schema::<Transcript>(schemas);

    let mut stt = operation("transcribe_voice", "Transcribe a WAV clip")
        .description(Some("Accepts a complete audio/wav clip with mono 16 kHz 16-bit PCM and at most 30 seconds of audio. An active current cloud_voice consent is required."))
        .request_body(Some(
            RequestBodyBuilder::new().required(Some(Required::True)).content(AUDIO_TYPE, wave_content()).build()
        ))
        .response("200", json_response("Transcript, language, script and confidence", "Transcript"));
    for (name, schema) in [("language", "Language"), ("script", "Script")] {
        stt = stt.parameter(
            ParameterBuilder::new()
                .name(name)
                .parameter_in(ParameterIn::Query)
                .schema(Some(Ref::from_schema_name(schema)))
                .build(),
        );
    }
    let tts = operation("synthesize_voice", "Synthesize text into WAV")
        .description(Some("Accepts 1 to 500 Unicode characters and returns mono 16 kHz 16-bit PCM WAV. An active current cloud_voice consent is required."))
        .request_body(Some(
            RequestBodyBuilder::new()
                .required(Some(Required::True))
                .content("application/json", Content::new(Some(Ref::from_schema_name("TtsRequest"))))
                .build()
        ))
        .response("200", ResponseBuilder::new().description("WAV audio").content(AUDIO_TYPE, wave_content()).build());
    doc.paths
        .paths
        .insert(STT_PATH.into(), PathItem::new(HttpMethod::Post, stt));
    doc.paths
        .paths
        .insert(TTS_PATH.into(), PathItem::new(HttpMethod::Post, tts));
}

fn operation(id: &str, summary: &str) -> OperationBuilder {
    let mut operation = OperationBuilder::new()
        .operation_id(Some(id))
        .summary(Some(summary))
        .security(SecurityRequirement::new("bearer", Vec::<String>::new()));
    for (status, description) in [
        ("400", "Invalid audio, text, locale or JSON"),
        ("401", "An active verified session is required"),
        (
            "403",
            "The current cloud_voice consent is missing or withdrawn",
        ),
        ("413", "Request exceeds the size or audio duration limit"),
        ("415", "Unsupported request media type"),
        ("429", "The daily voice quota is exhausted"),
        ("502", "Voice provider failed or returned invalid output"),
        (
            "503",
            "Voice disabled, service budget exhausted or usage unavailable",
        ),
        ("504", "Voice provider timed out"),
    ] {
        operation = operation.response(status, json_response(description, "ApiError"));
    }
    operation
}

fn wave_content() -> Content {
    let mut schema = Object::with_type(Type::String);
    schema.format = Some(SchemaFormat::KnownFormat(KnownFormat::Binary));
    Content::new(Some(schema))
}
