use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use moby_api::{AccessKeys, config::AccountConfig, openapi, router};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

async fn app_without_database() -> Router {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://moby@127.0.0.1/moby")
        .expect("valid test connection URL");
    pool.close().await;
    router(
        pool,
        AccountConfig::default(),
        AccessKeys::ephemeral().unwrap(),
        None,
    )
}

async fn get(app: Router, path: &str) -> Response {
    let request = Request::builder()
        .uri(path)
        .header("x-request-id", "untrusted-client-id")
        .body(Body::empty())
        .expect("valid HTTP request");
    let response = app.oneshot(request).await.expect("infallible router");
    let request_id = response.headers()["x-request-id"].to_str().unwrap();
    assert_eq!(Uuid::parse_str(request_id).unwrap().get_version_num(), 4);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    response
}

async fn body(response: Response) -> Value {
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_is_independent_of_database() {
    let response = get(app_without_database().await, "/health").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, json!({ "status": "ok" }));
}

#[tokio::test]
async fn ready_returns_503_when_database_is_unavailable() {
    let response = get(app_without_database().await, "/ready").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(response).await, json!({ "status": "not_ready" }));
}

#[tokio::test]
async fn openapi_endpoint_matches_release_artifact() {
    let generated = format!("{}\n", serde_json::to_string_pretty(&openapi()).unwrap());
    assert_eq!(
        generated,
        include_str!("../openapi.json"),
        "regenerate openapi.json using the command in README.md"
    );

    let response = get(app_without_database().await, "/openapi.json").await;
    assert_eq!(response.status(), StatusCode::OK);
    let served = body(response).await;
    assert_eq!(served, serde_json::from_str::<Value>(&generated).unwrap());
    assert!(served["paths"]["/ready"]["get"]["responses"]["503"].is_object());
    let agent = &served["paths"]["/v1/agent/turn"]["post"];
    assert!(agent["security"][0]["bearer"].is_array());
    assert_eq!(
        agent["responses"]["200"]["content"]["text/event-stream"]["x-sse-data-schema"]["$ref"],
        "#/components/schemas/AgentTurnResponse"
    );
    let schemas = &served["components"]["schemas"];
    assert_eq!(
        schemas["AgentTurnResponse"]["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        schemas["AgentMessage"]["oneOf"][0]["properties"]["kind"]["enum"],
        serde_json::json!(["user"])
    );
    assert_eq!(schemas["AgentTurnRequest"]["additionalProperties"], false);
}
