use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use moby_protocol::{AgentTurnRequest, AgentTurnResponse, ApiError, UsageInfo};
use schemars::{
    JsonSchema,
    generate::SchemaSettings,
    transform::{ReplaceConstValue, Transform},
};
use serde_json::{Value, json};
use tokio::time::Instant;
use utoipa::openapi::{
    Content, HttpMethod, OpenApi, Ref, RefOr, Schema, SecurityRequirement,
    extensions::ExtensionsBuilder,
    path::{OperationBuilder, PathItem},
    request_body::RequestBodyBuilder,
    response::ResponseBuilder,
    schema::{Object, Type},
};
use uuid::Uuid;

use crate::{
    http::AppState,
    sessions::routes::Authorized,
    usage::{self, UsageError},
};

use super::{
    Accounting, LlmProvider, MockProvider, SYSTEM_PROMPT, TURN_TIMEOUT, api_error, sse, validate,
};

const PATH: &str = "/v1/agent/turn";
const USAGE_PATH: &str = "/v1/usage";
const MAX_BODY: usize = 128 * 1024;
type ApiFailure = (StatusCode, Json<ApiError>);

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(PATH, post(turn))
        .route(USAGE_PATH, get(current_usage))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

async fn turn(
    State(state): State<AppState>,
    auth: Authorized,
    input: Result<Json<AgentTurnRequest>, JsonRejection>,
) -> Result<Response, ApiFailure> {
    let Json(request) = input.map_err(|rejection| {
        let (status, code, message) = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
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
            Json(api_error(code, message, auth.request_id.clone())),
        )
    })?;
    validate(&request).map_err(|message| {
        (
            StatusCode::BAD_REQUEST,
            Json(api_error(
                "invalid_history",
                message,
                auth.request_id.clone(),
            )),
        )
    })?;
    if !state.usage.agent_enabled {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(api_error(
                "agent_disabled",
                "Agent turns are paused",
                auth.request_id,
            )),
        ));
    }
    let permit = state.agent_circuit.enter(Instant::now()).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(api_error(
                "provider_circuit_open",
                "Agent provider is temporarily unavailable",
                auth.request_id.clone(),
            )),
        )
    })?;
    let id = Uuid::parse_str(&auth.request_id).expect("server-generated request ID");
    usage::reserve(
        &state.pool,
        state.usage,
        auth.identity.user_id,
        id,
        MockProvider.estimate(&request, SYSTEM_PROMPT),
    )
    .await
    .map_err(|error| usage_failure(error, auth.request_id.clone()))?;
    let accounting = Accounting {
        pool: state.pool,
        id,
        permit,
    };
    Ok(sse(
        &MockProvider,
        request,
        auth.request_id,
        TURN_TIMEOUT,
        Some(accounting),
    ))
}

async fn current_usage(
    State(state): State<AppState>,
    auth: Authorized,
) -> Result<Json<UsageInfo>, ApiFailure> {
    usage::current(&state.pool, state.usage, auth.identity.user_id)
        .await
        .map(Json)
        .map_err(|error| usage_failure(error, auth.request_id))
}

fn usage_failure(error: UsageError, request_id: String) -> ApiFailure {
    let (status, code, message) = match error {
        UsageError::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A verified session is required",
        ),
        UsageError::QuotaExceeded => (
            StatusCode::TOO_MANY_REQUESTS,
            "quota_exceeded",
            "The turn exceeds the remaining daily LLM quota",
        ),
        UsageError::BudgetExhausted => (
            StatusCode::SERVICE_UNAVAILABLE,
            "service_budget_exhausted",
            "The agent service budget is exhausted",
        ),
        UsageError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "usage_unavailable",
            "Agent usage could not be checked",
        ),
    };
    (status, Json(api_error(code, message, request_id)))
}

pub(crate) fn add_openapi(doc: &mut OpenApi) {
    let components = &mut doc.components.get_or_insert_with(Default::default).schemas;
    add_schema::<AgentTurnRequest>(components);
    add_schema::<AgentTurnResponse>(components);
    add_schema::<UsageInfo>(components);

    let mut stream = Content::new(Some(Object::with_type(Type::String)));
    stream.extensions = Some(
        ExtensionsBuilder::new()
            .add(
                "x-sse-data-schema",
                json!(Ref::from_schema_name("AgentTurnResponse")),
            )
            .build(),
    );
    stream.example = Some(json!(concat!(
        "data: {\"event\":\"speech_delta\",\"text\":\"Salom!\"}\n\n",
        "data: {\"event\":\"completed\",\"speech_text\":\"Salom!\",\"expression_hint\":\"happy\"}\n\n"
    )));
    let mut operation = OperationBuilder::new()
        .operation_id(Some("agent_turn"))
        .summary(Some("Run a stateless agent turn"))
        .description(Some(concat!(
            "Each SSE data field contains one AgentTurnResponse. Zero or more speech_delta ",
            "events precede exactly one completed, tool_call, or error event. Discard ",
            "provisional speech on tool_call, error, or EOF without completion. The desktop ",
            "executes a tool locally and submits the call and its matching result in the next ",
            "request. The API allows up to 128 KiB JSON, 64 messages, 64 resources, and 8 KiB ",
            "per message or tool result. The provider deadline is 15 seconds."
        )))
        .security(SecurityRequirement::new("bearer", Vec::<String>::new()))
        .request_body(Some(
            RequestBodyBuilder::new()
                .required(Some(utoipa::openapi::Required::True))
                .content(
                    "application/json",
                    Content::new(Some(Ref::from_schema_name("AgentTurnRequest"))),
                )
                .build(),
        ))
        .response(
            "200",
            ResponseBuilder::new()
                .description("Agent events")
                .content("text/event-stream", stream)
                .build(),
        );
    for (status, description) in [
        ("400", "Invalid JSON, history, tool or resource"),
        ("401", "An active verified session is required"),
        ("413", "Request body exceeds the size limit"),
        ("429", "The request exceeds the remaining daily LLM quota"),
        (
            "503",
            "Service paused, budget exhausted, circuit open, or session and usage data unavailable",
        ),
    ] {
        operation = operation.response(status, json_response(description, "ApiError"));
    }
    doc.paths
        .paths
        .insert(PATH.into(), PathItem::new(HttpMethod::Post, operation));
    let usage = OperationBuilder::new()
        .operation_id(Some("current_usage"))
        .summary(Some("Read the current account's remaining quota"))
        .description(Some("The daily window resets at midnight UTC. Pending reservations count against the remaining balance."))
        .security(SecurityRequirement::new("bearer", Vec::<String>::new()))
        .response("200", json_response("Remaining quota and the next UTC reset", "UsageInfo"))
        .response("401", json_response("An active verified session is required", "ApiError"))
        .response("503", json_response("Session or usage data unavailable", "ApiError"));
    doc.paths
        .paths
        .insert(USAGE_PATH.into(), PathItem::new(HttpMethod::Get, usage));
}

fn json_response(description: &str, schema: &str) -> utoipa::openapi::Response {
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            Content::new(Some(Ref::from_schema_name(schema))),
        )
        .build()
}

fn add_schema<T: JsonSchema>(components: &mut std::collections::BTreeMap<String, RefOr<Schema>>) {
    let schema = SchemaSettings::draft2020_12()
        .with(|settings| {
            settings.definitions_path = "/components/schemas".into();
            settings.meta_schema = None;
        })
        .into_generator()
        .into_root_schema_for::<T>();
    let mut root = serde_json::to_value(schema).expect("serializable protocol schema");
    let definitions = root
        .as_object_mut()
        .expect("agent schema object")
        .remove("components")
        .expect("agent schema definitions");
    for (name, schema) in definitions["schemas"]
        .as_object()
        .expect("component schemas")
    {
        components
            .entry(name.clone())
            .or_insert_with(|| utoipa_schema(schema.clone()));
    }
    components.insert(T::schema_name().into_owned(), utoipa_schema(root));
}

fn utoipa_schema(value: Value) -> RefOr<Schema> {
    let mut schema = schemars::Schema::try_from(value).expect("protocol schema");
    // Utoipa's schema model represents a constant as a single-value enum.
    ReplaceConstValue::default().transform(&mut schema);
    serde_json::from_value(schema.into()).expect("OpenAPI-compatible protocol schema")
}
