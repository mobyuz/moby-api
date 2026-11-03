use std::sync::Arc;

use futures_util::{StreamExt, stream};
use moby_protocol::{
    AgentMessage, AgentTurnRequest, AgentTurnResponse, Expression, Language, ResourceKind, Script,
    StableId, ToolCall, ToolName, ToolRequest, ToolResult,
};

use super::{LlmProvider, ProviderStream, call_id_used};

pub(crate) struct MockProvider;

impl LlmProvider for MockProvider {
    fn stream(&self, request: Arc<AgentTurnRequest>, _: &'static str) -> ProviderStream {
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
            return stream::iter([AgentTurnResponse::ToolCall {
                call: ToolCall {
                    call_id,
                    request: ToolRequest::GitStatus {
                        project_id: project.id.clone(),
                    },
                },
            }])
            .boxed();
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
            AgentTurnResponse::SpeechDelta { text: text.into() },
            AgentTurnResponse::Completed {
                speech_text: text.into(),
                display_text: None,
                expression_hint: if outcome == Some(false) {
                    Expression::Confused
                } else {
                    Expression::Happy
                },
            },
        ])
        .boxed()
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
