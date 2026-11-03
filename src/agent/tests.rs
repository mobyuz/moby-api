use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

use axum::body::{Body, HttpBody, to_bytes};
use futures_util::{
    StreamExt,
    stream::{self, BoxStream},
    task::noop_waker_ref,
};
use moby_protocol::{AgentTurnRequest, AgentTurnResponse};
use serde_json::{Value, json};

use super::{Accounting, LlmProvider, ProviderStream, TURN_TIMEOUT, sse, validate};
use crate::usage::Charge;

struct TestProvider(Mutex<Option<ProviderStream>>);

impl LlmProvider for TestProvider {
    fn estimate(&self, _: &AgentTurnRequest, _: &str) -> Charge {
        Charge {
            tokens: 1,
            cost_microusd: 0,
        }
    }

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

fn output(
    upstream: BoxStream<'static, AgentTurnResponse>,
    timeout: Duration,
    accounting: Option<Accounting>,
) -> Body {
    sse(
        &TestProvider(Mutex::new(Some(upstream.map(Into::into).boxed()))),
        request(),
        "test-request".into(),
        timeout,
        accounting,
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
        None,
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
        let result = events(output(upstream, Duration::from_millis(100), None)).await;
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
    let mut body = output(upstream, TURN_TIMEOUT, None);
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

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database in MOBY_USAGE_TEST_DATABASE_URL"]
async fn usage_holds_survive_timeout_and_disconnect() {
    use sqlx::postgres::PgPoolOptions;
    use tokio::time::Instant;
    use uuid::Uuid;

    use super::circuit::Circuit;
    use crate::{MIGRATOR, UsageConfig, usage};

    let database = std::env::var("MOBY_USAGE_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database)
        .await
        .unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, email_verified_at) VALUES ($1, $2, now())")
        .bind(user)
        .bind(format!("agent-hold-{user}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let circuit = Circuit::default();
    let limits = UsageConfig {
        daily_tokens: 200,
        ..UsageConfig::default()
    };
    let estimate = Charge {
        tokens: 60,
        cost_microusd: 0,
    };
    for disconnect in [false, true] {
        let id = Uuid::new_v4();
        usage::reserve(&pool, limits, user, id, estimate)
            .await
            .unwrap();
        let upstream = stream::iter([AgentTurnResponse::SpeechDelta {
            text: "provisional".into(),
        }])
        .chain(stream::pending())
        .boxed();
        let timeout = if disconnect {
            TURN_TIMEOUT
        } else {
            Duration::from_millis(100)
        };
        let mut body = output(
            upstream,
            timeout,
            Some(Accounting {
                pool: pool.clone(),
                id,
                permit: circuit.enter(Instant::now()).unwrap(),
            }),
        );
        if disconnect {
            std::future::poll_fn(|context| Pin::new(&mut body).poll_frame(context))
                .await
                .unwrap()
                .unwrap();
            drop(body);
        } else {
            let frames = events(body).await;
            assert_eq!(frames.last().unwrap()["error"]["code"], "agent_timeout");
        }
        assert_eq!(
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT actual_tokens FROM agent_usage_reservations WHERE id = $1"
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            None
        );
    }
    assert_eq!(
        usage::current(&pool, limits, user)
            .await
            .unwrap()
            .llm_tokens_remaining,
        80
    );
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
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
