use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::StatusCode,
    routing::post,
};
use futures_util::StreamExt;
use moby_protocol::{AgentMessage, AgentTurnRequest, AgentTurnResponse};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tracing::warn;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::{
    accounts::{
        AuthError, hit_bucket,
        routes::{Client, ErrorBody},
    },
    agent::{Llm, LlmProvider},
    http::{ApiFailure, AppState, api_error},
    usage::{self, DemoBudget, UsageError},
};

#[cfg(test)]
mod tests;
mod turnstile;

use turnstile::{Turnstile, VerifyError};

const PATH: &str = "/v1/demo/chat";
const MAX_TEXT_CHARS: usize = 280;
const MAX_TOKEN_BYTES: usize = 2048;
const MAX_REPLY_CHARS: usize = 240;
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(8);
const SYSTEM_PROMPT: &str = include_str!("demo/prompts/v2.txt");
// A guest reply is one short completion, so far fewer events than this are expected.
const MAX_PROVIDER_EVENTS: usize = 64;

#[derive(Clone)]
pub struct Demo {
    bot: Turnstile,
    budget: DemoBudget,
    ip_limit: i32,
    provider: Llm,
}

impl Demo {
    pub(crate) fn new(
        secret: String,
        hostname: &str,
        budget: DemoBudget,
        ip_limit: i32,
        provider: Llm,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            bot: Turnstile::new(secret, hostname)?,
            budget,
            ip_limit,
            provider,
        })
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct DemoRequest {
    #[schema(min_length = 1, max_length = 280)]
    text: String,
    #[schema(min_length = 1, max_length = 2048)]
    bot_token: String,
}

#[derive(Serialize, ToSchema)]
struct DemoReply {
    #[schema(max_length = 240)]
    reply: String,
}

#[derive(OpenApi)]
#[openapi(paths(chat), components(schemas(DemoRequest, DemoReply)))]
pub(crate) struct DemoDoc;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(PATH, post(chat))
        .layer(DefaultBodyLimit::max(8 * 1024))
}

#[utoipa::path(
    post, path = "/v1/demo/chat", request_body = DemoRequest,
    responses(
        (status = 200, description = "Short guest reply", body = DemoReply),
        (status = 400, description = "Invalid text, token or JSON", body = ErrorBody),
        (status = 403, description = "Bot verification rejected", body = ErrorBody),
        (status = 413, description = "Request exceeds 8 KiB", body = ErrorBody),
        (status = 429, description = "Guest IP rate limit reached", body = ErrorBody),
        (status = 502, description = "Guest provider failed or returned invalid output", body = ErrorBody),
        (status = 503, description = "Demo disabled, verification unavailable or guest budget exhausted", body = ErrorBody),
        (status = 504, description = "Guest provider timed out", body = ErrorBody)
    )
)]
async fn chat(
    State(state): State<AppState>,
    client: Client,
    body: Result<Json<DemoRequest>, JsonRejection>,
) -> Result<Json<DemoReply>, ApiFailure> {
    let fail = |status, code, message| {
        (
            status,
            Json(api_error(code, message, client.request_id.clone())),
        )
    };
    let unavailable = StatusCode::SERVICE_UNAVAILABLE;
    let demo = state
        .demo
        .as_ref()
        .ok_or_else(|| fail(unavailable, "demo_disabled", "The guest demo is disabled"))?;
    let Json(input) = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "Request body exceeds the size limit",
            )
        } else {
            fail(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "Expected a valid JSON request",
            )
        }
    })?;
    if input.text.trim().is_empty()
        || input.text.chars().take(MAX_TEXT_CHARS + 1).count() > MAX_TEXT_CHARS
    {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "invalid_text",
            "Send between 1 and 280 characters of text",
        ));
    }
    if input.bot_token.is_empty()
        || input.bot_token.len() > MAX_TOKEN_BYTES
        || !input.bot_token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "invalid_bot_token",
            "Send a valid bot verification token",
        ));
    }
    hit_bucket(&state.pool, &demo.bot.rate_bucket(client.ip), demo.ip_limit)
        .await
        .map_err(|error| match error {
            AuthError::RateLimited => fail(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many guest requests from this network",
            ),
            _ => fail(
                unavailable,
                "demo_unavailable",
                "Could not check the guest rate limit",
            ),
        })?;
    demo.bot
        .verify(&input.bot_token)
        .await
        .map_err(|error| match error {
            VerifyError::Rejected => fail(
                StatusCode::FORBIDDEN,
                "bot_verification_failed",
                "Complete the bot verification again",
            ),
            VerifyError::Unavailable => fail(
                unavailable,
                "bot_verification_unavailable",
                "Bot verification is temporarily unavailable",
            ),
        })?;

    let id = Uuid::parse_str(&client.request_id).expect("server-generated request ID");
    let request = Arc::new(AgentTurnRequest {
        messages: vec![AgentMessage::User { text: input.text }],
        available_tools: Vec::new(),
        resources: Vec::new(),
        language: None,
        script: None,
        local_time: None,
    });
    let provider = &demo.provider;
    usage::reserve_demo(
        &state.pool,
        demo.budget,
        id,
        provider.estimate(&request, SYSTEM_PROMPT),
    )
    .await
    .map_err(|error| match error {
        UsageError::BudgetExhausted => fail(
            unavailable,
            "demo_budget_exhausted",
            "The guest demo budget is exhausted",
        ),
        _ => fail(
            unavailable,
            "demo_unavailable",
            "Could not reserve the guest demo budget",
        ),
    })?;
    if usage::label(&state.pool, id, &provider.route())
        .await
        .is_err()
    {
        warn!(request_id = %client.request_id, "usage route could not be recorded");
    }
    // Dropping the stream cancels the provider request.
    let mut stream = provider
        .stream(request, SYSTEM_PROMPT)
        .take(MAX_PROVIDER_EVENTS);
    let output = timeout(PROVIDER_TIMEOUT, async {
        while let Some(event) = stream.next().await {
            if !matches!(event.response, AgentTurnResponse::SpeechDelta { .. }) {
                return Some(event);
            }
        }
        None
    })
    .await
    .map_err(|_| {
        fail(
            StatusCode::GATEWAY_TIMEOUT,
            "demo_timeout",
            "The guest provider timed out",
        )
    })?
    .ok_or_else(|| {
        fail(
            StatusCode::BAD_GATEWAY,
            "demo_provider_failed",
            "The guest provider returned no reply",
        )
    })?;
    if let Some(reason) = output.reason {
        warn!(request_id = %client.request_id, reason, "guest provider failed");
    }
    if let Some(served) = &output.served
        && usage::label(&state.pool, id, served).await.is_err()
    {
        warn!(request_id = %client.request_id, "usage route could not be recorded");
    }
    if let Some(charge) = output.charge {
        usage::settle(&state.pool, id, charge).await.map_err(|_| {
            fail(
                unavailable,
                "demo_unavailable",
                "Could not settle the guest demo budget",
            )
        })?;
    }
    match output.response {
        AgentTurnResponse::Completed { speech_text, .. }
            if !speech_text.trim().is_empty()
                && speech_text.chars().take(MAX_REPLY_CHARS + 1).count() <= MAX_REPLY_CHARS =>
        {
            Ok(Json(DemoReply { reply: speech_text }))
        }
        _ => Err(fail(
            StatusCode::BAD_GATEWAY,
            "demo_provider_failed",
            "The guest provider returned an invalid reply",
        )),
    }
}
