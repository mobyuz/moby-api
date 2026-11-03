use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

use axum::body::{Body, HttpBody, to_bytes};
use futures_util::{StreamExt, stream, task::noop_waker_ref};
use moby_protocol::{AgentTurnRequest, AgentTurnResponse};
use serde_json::{Value, json};

use super::{LlmProvider, ProviderStream, TURN_TIMEOUT, sse, validate};

struct TestProvider(Mutex<Option<ProviderStream>>);

impl LlmProvider for TestProvider {
    fn stream(&self, _: Arc<AgentTurnRequest>, prompt: &'static str) -> ProviderStream {
        assert!(prompt.starts_with("Moby agent prompt v1"));
        self.0.lock().unwrap().take().unwrap()
    }
}

fn request() -> AgentTurnRequest {
    serde_json::from_value(json!({
        "messages": [{"kind": "user", "text": "git status"}],
        "available_tools": ["git_status"],
        "resources": [{"kind": "project", "id": "demo", "display_name": "Demo"}]
    }))
    .unwrap()
}

fn output(upstream: ProviderStream, timeout: Duration) -> Body {
    sse(
        &TestProvider(Mutex::new(Some(upstream))),
        request(),
        "test-request".into(),
        timeout,
    )
    .into_body()
}

async fn events(body: Body) -> Vec<Value> {
    let bytes = to_bytes(body, 16 * 1024).await.unwrap();
    std::str::from_utf8(&bytes)
        .unwrap()
        .split_terminator("\n\n")
        .map(|line| serde_json::from_str(line.strip_prefix("data: ").unwrap()).unwrap())
        .collect()
}

fn upstream_tool() -> AgentTurnResponse {
    serde_json::from_value(json!({
        "event": "tool_call",
        "call": {
            "call_id": "test-call",
            "request": {"tool": "git_status", "project_id": "unadvertised"}
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn provider_tool_must_target_an_advertised_resource() {
    let result = events(output(
        stream::iter([
            upstream_tool(),
            AgentTurnResponse::SpeechDelta {
                text: "unexpected".into(),
            },
        ])
        .boxed(),
        TURN_TIMEOUT,
    ))
    .await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["event"], "error");
    assert_eq!(result[0]["error"]["code"], "invalid_provider_output");
    assert_eq!(result[0]["error"]["request_id"], "test-request");
}

#[tokio::test]
async fn timeout_and_premature_end_have_one_terminal_error() {
    let provisional = || AgentTurnResponse::SpeechDelta {
        text: "provisional".into(),
    };
    for (upstream, code) in [
        (
            stream::iter([provisional()])
                .chain(stream::pending())
                .boxed(),
            "agent_timeout",
        ),
        (
            stream::iter([provisional()]).boxed(),
            "provider_unavailable",
        ),
    ] {
        let result = events(output(upstream, Duration::from_millis(100))).await;
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["event"], "speech_delta");
        assert_eq!(result[1]["event"], "error");
        assert_eq!(result[1]["error"]["code"], code);
    }
}

#[tokio::test]
async fn dropping_sse_body_cancels_pending_provider() {
    let (done, mut canceled) = tokio::sync::oneshot::channel::<()>();
    let upstream = stream::iter([AgentTurnResponse::SpeechDelta {
        text: "provisional".into(),
    }])
    .chain(stream::poll_fn(move |_| {
        let _guard = &done;
        Poll::Pending
    }))
    .boxed();
    let mut body = output(upstream, TURN_TIMEOUT);
    let mut context = Context::from_waker(noop_waker_ref());
    assert!(matches!(
        Pin::new(&mut body).poll_frame(&mut context),
        Poll::Ready(Some(Ok(_)))
    ));
    assert!(Pin::new(&mut body).poll_frame(&mut context).is_pending());
    assert!(matches!(
        canceled.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    drop(body);
    assert!(matches!(
        canceled.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    ));
}

#[test]
fn history_requires_matching_call_result_and_registered_tool_resource() {
    let mut request = request();
    let AgentTurnResponse::ToolCall { mut call } = upstream_tool() else {
        unreachable!()
    };
    request
        .messages
        .push(moby_protocol::AgentMessage::ToolCall { call: call.clone() });
    assert!(validate(&request).is_err());
    call.request = moby_protocol::ToolRequest::GitStatus {
        project_id: request.resources[0].id.clone(),
    };
    request.messages[1] = moby_protocol::AgentMessage::ToolCall { call: call.clone() };
    let mut result = moby_protocol::ToolResult::Ok {
        call_id: moby_protocol::StableId::new("other-call").unwrap(),
        output: "synthetic".into(),
    };
    request
        .messages
        .push(moby_protocol::AgentMessage::ToolResult {
            result: result.clone(),
        });
    assert!(validate(&request).is_err());
    let moby_protocol::ToolResult::Ok { call_id, .. } = &mut result else {
        unreachable!()
    };
    *call_id = call.call_id;
    request.messages[2] = moby_protocol::AgentMessage::ToolResult { result };
    assert!(validate(&request).is_ok());
    request.available_tools.clear();
    assert!(validate(&request).is_err());
}
