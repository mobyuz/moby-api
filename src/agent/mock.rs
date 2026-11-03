use std::sync::Arc;

use futures_util::{StreamExt, stream};
use moby_protocol::{
    AgentMessage, AgentTurnRequest, AgentTurnResponse, Expression, Language, ResourceKind, Script,
    StableId, ToolCall, ToolName, ToolRequest, ToolResult,
};

use super::{LlmProvider, ProviderEvent, ProviderStream, call_id_used};
use crate::usage::Charge;

pub(crate) struct MockProvider;

const MOCK_OUTPUT_LIMIT: u32 = 512;

impl LlmProvider for MockProvider {
    fn estimate(&self, request: &AgentTurnRequest, prompt: &str) -> Charge {
        Charge {
            tokens: input_tokens(request, prompt) + MOCK_OUTPUT_LIMIT,
            cost_microusd: 0,
        }
    }

    fn stream(&self, request: Arc<AgentTurnRequest>, prompt: &'static str) -> ProviderStream {
        let input = input_tokens(&request, prompt);
        let last = request.messages.last();
        if let Some(AgentMessage::User { text }) = last
            && text.trim().eq_ignore_ascii_case("git status")
            && request.available_tools.contains(&ToolName::GitStatus)
            && let Some(project) = request
                .resources
                .iter()
                .find(|resource| resource.kind == ResourceKind::Project)
        {
            let call_id = (1..=request.messages.len() + 1)
                .map(|index| StableId::new(format!("mock-git-status-{index}")).unwrap())
                .find(|id| !call_id_used(&request.messages, id))
                .expect("at least one candidate is unused");
            let event = AgentTurnResponse::ToolCall {
                call: ToolCall {
                    call_id,
                    request: ToolRequest::GitStatus {
                        project_id: project.id.clone(),
                    },
                },
            };
            return stream::iter([terminal(event, input)]).boxed();
        }

        let outcome = last.and_then(|message| match message {
            AgentMessage::ToolResult { result } => Some(matches!(result, ToolResult::Ok { .. })),
            _ => None,
        });
        let text = match (request.language.unwrap_or(Language::Uz), request.script) {
            (Language::Uz, Some(Script::Cyrl)) => answer(
                outcome,
                "Салом! Моби синов режимида ишлаяпти.",
                "Восита натижаси қабул қилинди.",
                "Восита бажарилмади.",
            ),
            (Language::Uz, _) => answer(
                outcome,
                "Salom! Mobi sinov rejimida ishlayapti.",
                "Vosita natijasi qabul qilindi.",
                "Vosita bajarilmadi.",
            ),
            (Language::Ru, _) => answer(
                outcome,
                "Привет! Moby работает в тестовом режиме.",
                "Результат инструмента получен.",
                "Инструмент не выполнил действие.",
            ),
            (Language::En, _) => answer(
                outcome,
                "Hello! Moby is running in test mode.",
                "The tool result was received.",
                "The tool could not complete the action.",
            ),
        };
        stream::iter([
            AgentTurnResponse::SpeechDelta { text: text.into() }.into(),
            terminal(
                AgentTurnResponse::Completed {
                    speech_text: text.into(),
                    display_text: None,
                    expression_hint: if outcome == Some(false) {
                        Expression::Confused
                    } else {
                        Expression::Happy
                    },
                },
                input,
            ),
        ])
        .boxed()
    }
}

// The mock counts serialized UTF-8 bytes as deterministic token units.
fn input_tokens(request: &AgentTurnRequest, prompt: &str) -> u32 {
    let bytes = serde_json::to_vec(request)
        .expect("serializable agent request")
        .len();
    u32::try_from(bytes + prompt.len()).expect("bounded agent input")
}

fn terminal(response: AgentTurnResponse, input: u32) -> ProviderEvent {
    let bytes = serde_json::to_vec(&response)
        .expect("serializable agent response")
        .len();
    ProviderEvent {
        response,
        charge: Some(Charge {
            tokens: input + u32::try_from(bytes).expect("bounded mock output"),
            cost_microusd: 0,
        }),
    }
}

fn answer(
    outcome: Option<bool>,
    welcome: &'static str,
    success: &'static str,
    failure: &'static str,
) -> &'static str {
    match outcome {
        Some(true) => success,
        Some(false) => failure,
        None => welcome,
    }
}
