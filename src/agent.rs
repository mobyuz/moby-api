use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::response::{
    IntoResponse, Response,
    sse::{Event, Sse},
};
use futures_util::{
    StreamExt,
    stream::{self, BoxStream},
};
use moby_protocol::{
    AgentMessage, AgentTurnRequest, AgentTurnResponse, ResourceKind, StableId, ToolCall, ToolName,
    ToolRequest, ToolResult,
};
use sqlx::PgPool;
use tokio::time::{Instant, timeout_at};
use uuid::Uuid;

use crate::{
    http::api_error,
    usage::{self, Charge},
};

pub(crate) mod circuit;
mod mock;
pub(crate) mod routes;
#[cfg(test)]
mod tests;

pub(crate) const TURN_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const SYSTEM_PROMPT: &str = include_str!("agent/prompts/v1.txt");
const MAX_MESSAGES: usize = 64;
const MAX_RESOURCES: usize = 64;
const MAX_TEXT_BYTES: usize = 8 * 1024;
const MAX_DELTAS: usize = 32;

pub(crate) type ProviderStream = BoxStream<'static, ProviderEvent>;

pub(crate) struct ProviderEvent {
    pub(crate) response: AgentTurnResponse,
    pub(crate) charge: Option<Charge>,
}

impl From<AgentTurnResponse> for ProviderEvent {
    fn from(response: AgentTurnResponse) -> Self {
        Self {
            response,
            charge: None,
        }
    }
}

pub(crate) trait LlmProvider {
    fn estimate(&self, request: &AgentTurnRequest, prompt: &str) -> Charge;

    // The returned stream owns any provider work, which ends when it is dropped.
    // Report exact usage on the terminal event when the provider knows it.
    fn stream(&self, request: Arc<AgentTurnRequest>, prompt: &'static str) -> ProviderStream;
}

pub(crate) use mock::MockProvider;

pub(crate) fn validate(request: &AgentTurnRequest) -> Result<(), &'static str> {
    if !(1..=MAX_MESSAGES).contains(&request.messages.len()) {
        return Err("Send between 1 and 64 messages");
    }
    if request.resources.len() > MAX_RESOURCES {
        return Err("Send at most 64 resources");
    }
    for (index, resource) in request.resources.iter().enumerate() {
        let label = &resource.display_name;
        if label.trim().is_empty()
            || label.chars().take(101).count() > 100
            || label.chars().any(char::is_control)
            || request.resources[..index]
                .iter()
                .any(|prior| prior.kind == resource.kind && prior.id == resource.id)
        {
            return Err("Use unique resource IDs per kind and valid display names");
        }
    }
    for (index, tool) in request.available_tools.iter().enumerate() {
        if request.available_tools[..index].contains(tool) {
            return Err("Advertise each tool once");
        }
    }

    enum Next<'a> {
        User,
        Reply,
        Result(&'a StableId),
    }
    let mut next = Next::User;
    for (index, message) in request.messages.iter().enumerate() {
        next = match (next, message) {
            (Next::User, AgentMessage::User { text }) if valid_text(text) => Next::Reply,
            (Next::Reply, AgentMessage::Assistant { text }) if valid_text(text) => Next::User,
            (Next::Reply, AgentMessage::ToolCall { call }) => {
                if call_id_used(&request.messages[..index], &call.call_id) {
                    return Err("Use a unique call_id for each tool call");
                }
                if !advertised(request, &call.request) {
                    return Err("Tool calls must use advertised tools and resource IDs");
                }
                Next::Result(&call.call_id)
            }
            (Next::Result(expected), AgentMessage::ToolResult { result })
                if matches_result(expected, result) =>
            {
                Next::Reply
            }
            _ => return Err("Invalid message order, text, or tool result"),
        };
    }
    if !matches!(next, Next::Reply) {
        return Err("End the history with a user message or a matching tool result");
    }
    Ok(())
}

fn valid_text(text: &str) -> bool {
    text.len() <= MAX_TEXT_BYTES && !text.trim().is_empty()
}

fn matches_result(expected: &StableId, result: &ToolResult) -> bool {
    match result {
        ToolResult::Ok { call_id, output } => call_id == expected && output.len() <= MAX_TEXT_BYTES,
        ToolResult::Error { call_id, .. } => call_id == expected,
    }
}

fn call_id_used(messages: &[AgentMessage], id: &StableId) -> bool {
    messages
        .iter()
        .any(|message| matches!(message, AgentMessage::ToolCall { call } if &call.call_id == id))
}

fn advertised(request: &AgentTurnRequest, tool: &ToolRequest) -> bool {
    let has = |kind, id: &StableId| {
        request
            .resources
            .iter()
            .any(|resource| resource.kind == kind && &resource.id == id)
    };
    let (name, resources_valid) = match tool {
        ToolRequest::GitStatus { project_id } => {
            (ToolName::GitStatus, has(ResourceKind::Project, project_id))
        }
        ToolRequest::OpenApp { app_id } => (ToolName::OpenApp, has(ResourceKind::App, app_id)),
        ToolRequest::OpenProject { project_id, app_id } => (
            ToolName::OpenProject,
            has(ResourceKind::Project, project_id)
                && app_id.as_ref().is_none_or(|id| has(ResourceKind::App, id)),
        ),
        ToolRequest::SearchFiles { root_id, .. } => (
            ToolName::SearchFiles,
            has(ResourceKind::SearchRoot, root_id),
        ),
        ToolRequest::CreateReminder { .. } => (ToolName::CreateReminder, true),
    };
    resources_valid && request.available_tools.contains(&name)
}

pub(crate) struct Accounting {
    pub(crate) pool: PgPool,
    pub(crate) id: Uuid,
    pub(crate) permit: circuit::Permit,
}

struct Turn {
    provider: ProviderStream,
    request: Arc<AgentTurnRequest>,
    request_id: String,
    deadline: Instant,
    speech_bytes: usize,
    deltas: usize,
    accounting: Option<Accounting>,
}

impl Turn {
    async fn next(&mut self) -> AgentTurnResponse {
        let next = if Instant::now() >= self.deadline {
            None
        } else {
            timeout_at(self.deadline, self.provider.next()).await.ok()
        };
        let (response, charge) = match next {
            Some(Some(event)) => (self.validate_output(event.response), event.charge),
            Some(None) => (
                self.error("provider_unavailable", "Agent response was interrupted"),
                None,
            ),
            None => (
                self.error("agent_timeout", "Agent response timed out"),
                None,
            ),
        };
        if !matches!(response, AgentTurnResponse::SpeechDelta { .. })
            && let Some(accounting) = self.accounting.take()
        {
            accounting.permit.finish(
                !matches!(response, AgentTurnResponse::Error { .. }),
                Instant::now(),
            );
            if let Some(charge) = charge
                && usage::settle(&accounting.pool, accounting.id, charge)
                    .await
                    .is_err()
            {
                return self.error("usage_unavailable", "Agent usage could not be settled");
            }
        }
        response
    }

    fn validate_output(&mut self, response: AgentTurnResponse) -> AgentTurnResponse {
        let error = match response {
            AgentTurnResponse::SpeechDelta { ref text } => {
                self.deltas += 1;
                self.speech_bytes += text.len();
                if self.deltas <= MAX_DELTAS && self.speech_bytes <= MAX_TEXT_BYTES {
                    return response;
                }
                (
                    "invalid_provider_output",
                    "Agent response exceeded its limits",
                )
            }
            AgentTurnResponse::ToolCall { ref call } if self.valid_tool_call(call) => {
                return response;
            }
            AgentTurnResponse::Completed {
                ref speech_text,
                ref display_text,
                ..
            } if valid_text(speech_text)
                && display_text.as_ref().is_none_or(|text| valid_text(text)) =>
            {
                return response;
            }
            AgentTurnResponse::Error { .. } => {
                ("provider_unavailable", "Agent response was interrupted")
            }
            _ => (
                "invalid_provider_output",
                "Agent response failed validation",
            ),
        };
        self.error(error.0, error.1)
    }

    fn error(&self, code: &str, message: &str) -> AgentTurnResponse {
        AgentTurnResponse::Error {
            error: api_error(code, message, self.request_id.clone()),
        }
    }

    fn valid_tool_call(&self, call: &ToolCall) -> bool {
        advertised(&self.request, &call.request)
            && !call_id_used(&self.request.messages, &call.call_id)
    }
}

pub(crate) fn sse(
    provider: &impl LlmProvider,
    request: AgentTurnRequest,
    request_id: String,
    timeout: Duration,
    accounting: Option<Accounting>,
) -> Response {
    let request = Arc::new(request);
    let deadline = Instant::now() + timeout;
    let turn = Turn {
        provider: provider.stream(Arc::clone(&request), SYSTEM_PROMPT),
        request,
        request_id,
        deadline,
        speech_bytes: 0,
        deltas: 0,
        accounting,
    };
    Sse::new(stream::unfold(Some(turn), |turn| async {
        let mut turn = turn?;
        let event = turn.next().await;
        let continuing = matches!(event, AgentTurnResponse::SpeechDelta { .. });
        let frame = Event::default()
            .json_data(event)
            .expect("agent wire events are JSON serializable");
        Some((Ok::<_, Infallible>(frame), continuing.then_some(turn)))
    }))
    .into_response()
}
