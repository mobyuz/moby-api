use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    Form,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, header},
};
use serde_json::{Value, json};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tokio::net::TcpListener;
use tower::ServiceExt;
use url::Url;

use super::*;
use crate::{
    AccessKeys, MIGRATOR, UsageConfig,
    config::AccountConfig,
    router_with_demo,
    usage::{Charge, Work},
};

const SECRET: &str = "synthetic-test-turnstile-key";

#[derive(Default)]
struct Site {
    calls: AtomicUsize,
    used: Mutex<HashSet<String>>,
}

async fn siteverify(
    State(site): State<Arc<Site>>,
    Form(mut form): Form<HashMap<String, String>>,
) -> Json<Value> {
    site.calls.fetch_add(1, Ordering::Relaxed);
    assert_eq!(form.remove("secret").as_deref(), Some(SECRET));
    let token = form.remove("response").unwrap();
    assert!(
        form.is_empty(),
        "Siteverify should receive only the secret and token"
    );
    match token.as_str() {
        "wrong-host" => {
            Json(json!({"success":true,"hostname":"other.example.test","action":"demo_chat"}))
        }
        "wrong-action" => {
            Json(json!({"success":true,"hostname":"demo.example.test","action":"other"}))
        }
        "site-unavailable" => Json(json!({"success":false,"error-codes":["internal-error"]})),
        _ => {
            let success = token.starts_with("pass-") && site.used.lock().unwrap().insert(token);
            Json(json!({"success":success,"hostname":"demo.example.test","action":"demo_chat"}))
        }
    }
}

async fn request(app: &Router, peer: u8, text: &str, token: &str, status: StatusCode) -> Value {
    let request = Request::post(PATH)
        .extension(ConnectInfo(SocketAddr::from(([198, 51, 100, peer], 41000))))
        .header("x-forwarded-for", format!("203.0.113.{}", token.len()))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"text":text,"bot_token":token}).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let result: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    if status.is_client_error() || status.is_server_error() {
        assert_eq!(result["request_id"], request_id);
    }
    result
}

async fn budgets(pool: &PgPool) -> Vec<(bool, bool, i64, i64)> {
    sqlx::query_as(
        "SELECT guest, monthly, cost_microusd, units FROM usage_budget_periods ORDER BY guest, monthly",
    ).fetch_all(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database in MOBY_DEMO_TEST_DATABASE_URL"]
async fn guest_bot_rate_and_budget_round_trip() {
    let url = std::env::var("MOBY_DEMO_TEST_DATABASE_URL").expect("disposable database URL");
    let options: PgConnectOptions = url.parse().unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    let schema = format!("demo_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options.options([("search_path", &schema)]))
        .await
        .unwrap();
    MIGRATOR.run(&pool).await.unwrap();

    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, email_verified_at) VALUES ($1, 'demo-budget@example.test', now())")
        .bind(user).execute(&pool).await.unwrap();
    let user_limits = UsageConfig {
        daily_budget_microusd: 100,
        monthly_budget_microusd: 100,
        ..UsageConfig::default()
    };
    let account_id = Uuid::new_v4();
    usage::reserve(
        &pool,
        user_limits,
        user,
        account_id,
        Work::Agent,
        Charge {
            units: 40,
            cost_microusd: 70,
        },
    )
    .await
    .unwrap();
    usage::settle(
        &pool,
        account_id,
        Charge {
            units: 20,
            cost_microusd: 40,
        },
    )
    .await
    .unwrap();

    let site = Arc::new(Site::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
        Url::parse(&format!("http://{}/verify", listener.local_addr().unwrap())).unwrap();
    let site_app = Router::new()
        .route("/verify", post(siteverify))
        .with_state(site.clone());
    let site_task = tokio::spawn(async move { axum::serve(listener, site_app).await.unwrap() });
    let bot = Turnstile::with_endpoint(SECRET.into(), "demo.example.test".into(), endpoint, false)
        .unwrap();
    assert_eq!(
        bot.rate_bucket("2001:db8::1".parse().unwrap()),
        bot.rate_bucket("2001:db8::2".parse().unwrap())
    );
    assert_ne!(
        bot.rate_bucket("2001:db8::1".parse().unwrap()),
        bot.rate_bucket("2001:db8:0:1::1".parse().unwrap())
    );

    let text = "git status";
    let estimate = Llm::mock().estimate(
        &AgentTurnRequest {
            messages: vec![AgentMessage::User { text: text.into() }],
            available_tools: vec![],
            resources: vec![],
            language: None,
            script: None,
        },
        SYSTEM_PROMPT,
    );
    let demo = Demo {
        bot,
        budget: DemoBudget {
            daily_units: estimate.units,
            daily_cost_microusd: 0,
            monthly_cost_microusd: 0,
        },
        ip_limit: 2,
        provider: Llm::mock(),
    };
    let keys = AccessKeys::ephemeral().unwrap();
    let app = router_with_demo(
        pool.clone(),
        AccountConfig::default(),
        keys.clone(),
        None,
        user_limits,
        Some(demo),
    );
    let disabled = router_with_demo(
        pool.clone(),
        AccountConfig::default(),
        keys,
        None,
        user_limits,
        None,
    );
    assert_eq!(
        request(
            &disabled,
            1,
            text,
            "pass-disabled",
            StatusCode::SERVICE_UNAVAILABLE
        )
        .await["code"],
        "demo_disabled"
    );
    assert_eq!(
        request(&app, 1, " ", "pass-empty", StatusCode::BAD_REQUEST).await["code"],
        "invalid_text"
    );
    assert_eq!(site.calls.load(Ordering::Relaxed), 0);

    for token in ["reject", "wrong-host"] {
        assert_eq!(
            request(&app, 1, text, token, StatusCode::FORBIDDEN).await["code"],
            "bot_verification_failed"
        );
    }
    assert_eq!(
        request(
            &app,
            1,
            text,
            "pass-rate-limited",
            StatusCode::TOO_MANY_REQUESTS
        )
        .await["code"],
        "rate_limited"
    );
    assert_eq!(site.calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        request(&app, 2, text, "wrong-action", StatusCode::FORBIDDEN).await["code"],
        "bot_verification_failed"
    );
    assert_eq!(
        request(&app, 2, text, "pass-one", StatusCode::OK).await,
        json!({"reply":"Salom! Mobi sinov rejimida ishlayapti."})
    );
    assert_eq!(
        request(&app, 3, text, "pass-one", StatusCode::FORBIDDEN).await["code"],
        "bot_verification_failed"
    );
    assert_eq!(
        request(
            &app,
            3,
            text,
            "site-unavailable",
            StatusCode::SERVICE_UNAVAILABLE
        )
        .await["code"],
        "bot_verification_unavailable"
    );
    assert_eq!(
        request(
            &app,
            4,
            text,
            "pass-budget",
            StatusCode::SERVICE_UNAVAILABLE
        )
        .await["code"],
        "demo_budget_exhausted"
    );
    assert_eq!(site.calls.load(Ordering::Relaxed), 7);

    type GuestRecord = (Option<Uuid>, i64, Option<i64>, Option<i64>);
    let guest_records: Vec<GuestRecord> = sqlx::query_as(
        "SELECT user_id, reserved_units, actual_units, actual_cost_microusd FROM usage_reservations WHERE meter = 'demo'",
    ).fetch_all(&pool).await.unwrap();
    let [(None, reserved, Some(used), Some(0))] = guest_records.as_slice() else {
        panic!("expected exactly one settled anonymous demo reservation");
    };
    assert_eq!(*reserved, i64::from(estimate.units));
    assert!((1..*reserved).contains(used));
    assert_eq!(
        budgets(&pool).await,
        [
            (false, false, 40, 0),
            (false, true, 40, 0),
            (true, false, 0, *used)
        ]
    );
    let ip_buckets: Vec<String> =
        sqlx::query_scalar("SELECT bucket FROM auth_rate_limits WHERE bucket LIKE 'demo:ip:%'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ip_buckets.len(), 4);
    assert!(ip_buckets.iter().all(
        |bucket| bucket.strip_prefix("demo:ip:").is_some_and(|hash| {
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    ));

    let budget = DemoBudget {
        daily_units: u32::try_from(*used).unwrap() + 70,
        daily_cost_microusd: 15,
        monthly_cost_microusd: 15,
    };
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let upper = Charge {
        units: 60,
        cost_microusd: 10,
    };
    let (left, right) = tokio::join!(
        usage::reserve_demo(&pool, budget, a, upper),
        usage::reserve_demo(&pool, budget, b, upper),
    );
    let winner = match (left, right) {
        (Ok(()), Err(UsageError::BudgetExhausted)) => a,
        (Err(UsageError::BudgetExhausted), Ok(())) => b,
        other => panic!("expected one guest budget reservation, got {other:?}"),
    };
    let actual = Charge {
        units: 12,
        cost_microusd: 3,
    };
    usage::settle(&pool, winner, actual).await.unwrap();
    usage::settle(&pool, winner, actual).await.unwrap();
    let low_month = DemoBudget {
        monthly_cost_microusd: 5,
        ..budget
    };
    assert_eq!(
        usage::reserve_demo(
            &pool,
            low_month,
            Uuid::new_v4(),
            Charge {
                units: 1,
                cost_microusd: 4
            }
        )
        .await,
        Err(UsageError::BudgetExhausted)
    );
    assert_eq!(
        budgets(&pool).await,
        [
            (false, false, 40, 0),
            (false, true, 40, 0),
            (true, false, 3, *used + 12),
            (true, true, 3, 0)
        ]
    );

    site_task.abort();
    drop(app);
    drop(disabled);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
