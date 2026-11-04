use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::{
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;

use super::*;
use crate::{AccessKeys, MIGRATOR, config::AccountConfig, router_with_usage, sessions};

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, email_verified_at) VALUES ($1, $2, now())")
        .bind(id)
        .bind(format!("quota-{id}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    id
}

fn charge(units: u32, cost_microusd: i64) -> Charge {
    Charge {
        units,
        cost_microusd,
    }
}

async fn balances(pool: &PgPool) -> Vec<(bool, i64)> {
    sqlx::query_as("SELECT monthly, cost_microusd FROM usage_budget_periods ORDER BY monthly")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn api(app: Router, method: &str, path: &str, token: &str, expected: StatusCode) -> Value {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"messages":[{"kind":"user","text":"Synthetic turn"}],"available_tools":[],"resources":[]}).to_string(),
        ))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), expected);
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    if expected.is_client_error() || expected.is_server_error() {
        assert_eq!(value["request_id"], request_id);
    }
    value
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database in MOBY_USAGE_TEST_DATABASE_URL"]
async fn usage_limits_round_trip() {
    let url = std::env::var("MOBY_USAGE_TEST_DATABASE_URL").expect("disposable database URL");
    let connection: PgConnectOptions = url.parse().unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connection.clone())
        .await
        .unwrap();
    let schema = format!("usage_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let connection = connection.options([("search_path", &schema)]);
    let mut pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(connection.clone())
        .await
        .unwrap();
    let limits = UsageConfig {
        daily_tokens: 100,
        ..UsageConfig::default()
    };
    let b4 = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version <= 5)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    b4.run(&pool).await.unwrap();
    let previous_user = user(&pool).await;
    let previous_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_usage_reservations (id, user_id, utc_day, reserved_tokens, reserved_cost_microusd)
         VALUES ($1, $2, (now() AT TIME ZONE 'UTC')::date, 7, 0)",
    )
    .bind(previous_id)
    .bind(previous_user)
    .execute(&pool)
    .await
    .unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    assert_eq!(
        current(&pool, limits, previous_user)
            .await
            .unwrap()
            .llm_tokens_remaining,
        93
    );
    settle(&pool, previous_id, charge(2, 0)).await.unwrap();
    let route = Route {
        provider: "openrouter",
        model: Some("vendor/model".into()),
    };
    label(&pool, previous_id, &route).await.unwrap();
    assert_eq!(
        sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT provider, model FROM usage_reservations WHERE id = $1"
        )
        .bind(previous_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        (Some("openrouter".into()), Some("vendor/model".into()))
    );
    assert_eq!(
        current(&pool, limits, previous_user)
            .await
            .unwrap()
            .llm_tokens_remaining,
        98
    );

    let first = user(&pool).await;
    let second = user(&pool).await;
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let (left, right) = tokio::join!(
        reserve(&pool, limits, first, a, Work::Agent, charge(80, 0)),
        reserve(&pool, limits, first, b, Work::Agent, charge(80, 0)),
    );
    let held = match (left, right) {
        (Ok(()), Err(UsageError::QuotaExceeded)) => a,
        (Err(UsageError::QuotaExceeded), Ok(())) => b,
        other => panic!("expected one quota reservation, got {other:?}"),
    };
    assert_eq!(
        current(&pool, limits, first)
            .await
            .unwrap()
            .llm_tokens_remaining,
        20
    );
    assert_eq!(
        current(&pool, limits, second)
            .await
            .unwrap()
            .llm_tokens_remaining,
        100
    );

    // A turn without a terminal usage report retains its hold on a fresh pool.
    pool.close().await;
    pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(connection)
        .await
        .unwrap();
    let pending = current(&pool, limits, first).await.unwrap();
    assert_eq!(pending.llm_tokens_remaining, 20);
    assert!(pending.reset_at.as_str().ends_with("T00:00:00Z"));
    let (left, right) = tokio::join!(
        settle(&pool, held, charge(30, 0)),
        settle(&pool, held, charge(30, 0)),
    );
    assert_eq!((left, right), (Ok(()), Ok(())));
    assert_eq!(
        settle(&pool, held, charge(31, 0)).await,
        Err(UsageError::Unavailable)
    );
    assert_eq!(
        current(&pool, limits, first)
            .await
            .unwrap()
            .llm_tokens_remaining,
        70
    );
    sqlx::query("UPDATE usage_reservations SET utc_day = utc_day - 1 WHERE id = $1")
        .bind(held)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        current(&pool, limits, first)
            .await
            .unwrap()
            .llm_tokens_remaining,
        100
    );

    let paid = UsageConfig {
        daily_tokens: 1000,
        daily_budget_microusd: 100,
        monthly_budget_microusd: 100,
        ..UsageConfig::default()
    };
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let (left, right) = tokio::join!(
        reserve(&pool, paid, first, a, Work::Agent, charge(40, 60)),
        reserve(&pool, paid, second, b, Work::Agent, charge(40, 60)),
    );
    let (winner, owner) = match (left, right) {
        (Ok(()), Err(UsageError::BudgetExhausted)) => (a, first),
        (Err(UsageError::BudgetExhausted), Ok(())) => (b, second),
        other => panic!("expected one global budget reservation, got {other:?}"),
    };
    assert_eq!(balances(&pool).await, [(false, 60), (true, 60)]);
    assert_eq!(
        settle(&pool, winner, charge(10, 61)).await,
        Err(UsageError::Unavailable)
    );
    assert_eq!(balances(&pool).await, [(false, 60), (true, 60)]);
    assert_eq!(settle(&pool, winner, charge(10, 20)).await, Ok(()));
    assert_eq!(settle(&pool, winner, charge(10, 20)).await, Ok(()));
    let low_month = UsageConfig {
        monthly_budget_microusd: 35,
        ..paid
    };
    assert_eq!(
        reserve(
            &pool,
            low_month,
            first,
            Uuid::new_v4(),
            Work::Agent,
            charge(5, 30)
        )
        .await,
        Err(UsageError::BudgetExhausted)
    );
    assert_eq!(balances(&pool).await, [(false, 20), (true, 20)]);
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT user_id FROM usage_reservations WHERE id = $1"
        )
        .bind(winner)
        .fetch_one(&pool)
        .await
        .unwrap(),
        None
    );
    assert_eq!(balances(&pool).await, [(false, 20), (true, 20)]);

    let account = user(&pool).await;
    let keys = AccessKeys::ephemeral().unwrap();
    let mut tx = pool.begin().await.unwrap();
    let pair = sessions::create_session(&mut tx, &keys, account, None)
        .await
        .unwrap_or_else(|_| panic!("could not create a synthetic session"));
    tx.commit().await.unwrap();
    let pair = serde_json::to_value(pair).unwrap();
    let token = pair["access_token"].as_str().unwrap();
    let limited = router_with_usage(
        pool.clone(),
        AccountConfig::default(),
        keys.clone(),
        None,
        limits,
    );
    let response = api(
        limited.clone(),
        "POST",
        "/v1/agent/turn",
        token,
        StatusCode::TOO_MANY_REQUESTS,
    )
    .await;
    assert_eq!(response["code"], "quota_exceeded");
    let usage = api(limited, "GET", "/v1/usage", token, StatusCode::OK).await;
    assert_eq!(usage["llm_tokens_remaining"], 100);
    let paused = router_with_usage(
        pool.clone(),
        AccountConfig::default(),
        keys,
        None,
        UsageConfig {
            agent_enabled: false,
            ..UsageConfig::default()
        },
    );
    let response = api(
        paused,
        "POST",
        "/v1/agent/turn",
        token,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(response["code"], "agent_disabled");
    assert_eq!(balances(&pool).await, [(false, 20), (true, 20)]);

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
use std::borrow::Cow;
