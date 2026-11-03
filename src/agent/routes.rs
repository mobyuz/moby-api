use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, rejection::JsonRejection},
    http::StatusCode,
    response::Response,
    routing::post,
};
use moby_protocol::{AgentTurnRequest, AgentTurnResponse, ApiError};
use schemars::{
    JsonSchema,
    generate::SchemaSettings,
    transform::{ReplaceConstValue, Transform},
};
use serde_json::{Value, json};
use utoipa::openapi::{
    Content, HttpMethod, OpenApi, Ref, RefOr, Schema, SecurityRequirement,
    extensions::ExtensionsBuilder,
    path::{OperationBuilder, PathItem},
    request_body::RequestBodyBuilder,
    response::ResponseBuilder,
    schema::{Object, Type},
};

use crate::{http::AppState, sessions::routes::Authorized};

use super::{MockProvider, TURN_TIMEOUT, api_error, sse, validate};

const PATH: &str = "/v1/agent/turn";
const MAX_BODY: usize = 128 * 1024;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(PATH, post(turn))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

async fn turn(
    auth: Authorized,
    input: Result<Json<AgentTurnRequest>, JsonRejection>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
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
    Ok(sse(&MockProvider, request, auth.request_id, TURN_TIMEOUT))
}

pub(crate) fn add_openapi(doc: &mut OpenApi) {
    let components = &mut doc.components.get_or_insert_with(Default::default).schemas;
    add_schema::<AgentTurnRequest>(components);
    add_schema::<AgentTurnResponse>(components);

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
        ("503", "Session service unavailable"),
    ] {
        operation = operation.response(
            status,
            ResponseBuilder::new()
                .description(description)
                .content(
                    "application/json",
                    Content::new(Some(Ref::from_schema_name("ApiError"))),
                )
                .build(),
        );
    }
    doc.paths
        .paths
        .insert(PATH.into(), PathItem::new(HttpMethod::Post, operation));
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
