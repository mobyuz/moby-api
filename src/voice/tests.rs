use std::{cell::Cell, net::SocketAddr};

use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Method, Request},
};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

use super::*;
use crate::{
    AccessKeys, MIGRATOR, UsageConfig, accounts::Accounts, config::AccountConfig,
    router_with_usage, sessions,
};

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    media: &str,
    body: impl Into<Body>,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41000))))
        .header(header::CONTENT_TYPE, media);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(request.body(body.into()).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap()).unwrap()
}

async fn expect_error(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = json_body(response).await;
    assert_eq!(body["code"], code);
    assert_eq!(body["request_id"], request_id);
}

async fn consent(app: &Router, token: &str, accepted: bool) {
    let response = call(
        app,
        Method::POST,
        "/v1/consents",
        Some(token),
        "application/json",
        json!({"type":"cloud_voice","version":"dev-1","accepted":accepted}).to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

async fn expect_no_dispatch(
    state: &AppState,
    auth: &Authorized,
    charge: Charge,
    status: StatusCode,
    code: &str,
) {
    let invoked = Cell::new(false);
    let result = execute(state, auth, Work::Stt("dev-1"), charge, || {
        invoked.set(true);
        async { Ok(((), 0)) }
    })
    .await;
    assert!(!invoked.get());
    let (response, Json(error)) = result.unwrap_err();
    assert_eq!(response, status);
    assert_eq!(error.code, code);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database in MOBY_VOICE_TEST_DATABASE_URL"]
async fn voice_consent_quota_and_locale_round_trip() {
    let database = std::env::var("MOBY_VOICE_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database)
        .await
        .unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, email_verified_at) VALUES ($1, $2, now())")
        .bind(user_id)
        .bind(format!("voice-{user_id}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let keys = AccessKeys::ephemeral().unwrap();
    let mut transaction = pool.begin().await.unwrap();
    let pair = sessions::create_session(&mut transaction, &keys, user_id, None)
        .await
        .unwrap_or_else(|_| panic!("could not create a synthetic session"));
    transaction.commit().await.unwrap();
    let pair = serde_json::to_value(pair).unwrap();
    let token = pair["access_token"].as_str().unwrap();
    let limits = UsageConfig {
        voice_enabled: true,
        daily_stt_seconds: 4,
        daily_tts_chars: 21,
        ..UsageConfig::default()
    };
    let config = AccountConfig::default();
    let app = router_with_usage(pool.clone(), config.clone(), keys.clone(), None, limits);
    let disabled = router_with_usage(
        pool.clone(),
        config.clone(),
        keys.clone(),
        None,
        UsageConfig::default(),
    );
    let state = AppState {
        pool: pool.clone(),
        accounts: Accounts::new(pool.clone(), config),
        access_keys: keys,
        google: None,
        usage: limits,
        agent_circuit: Default::default(),
        demo: None,
    };
    let mut auth = Authorized {
        identity: sessions::Identity {
            user_id,
            session_id: Uuid::new_v4(),
            installation_id: Uuid::new_v4(),
        },
        request_id: Uuid::new_v4().to_string(),
    };
    let clip = wave::encode(vec![0; 1600]).unwrap();
    let greeting = json!({"text":"Salom"}).to_string();
    let free = Charge {
        units: 1,
        cost_microusd: 0,
    };

    expect_error(
        call(&app, Method::POST, STT_PATH, None, AUDIO_TYPE, clip.clone()).await,
        StatusCode::UNAUTHORIZED,
        "unauthorized",
    )
    .await;
    expect_error(
        call(
            &disabled,
            Method::POST,
            STT_PATH,
            Some(token),
            AUDIO_TYPE,
            clip.clone(),
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE,
        "voice_disabled",
    )
    .await;
    expect_error(
        call(
            &app,
            Method::POST,
            STT_PATH,
            Some(token),
            AUDIO_TYPE,
            clip.clone(),
        )
        .await,
        StatusCode::FORBIDDEN,
        "voice_consent_required",
    )
    .await;
    expect_no_dispatch(
        &state,
        &auth,
        free,
        StatusCode::FORBIDDEN,
        "voice_consent_required",
    )
    .await;
    sqlx::query(
        "INSERT INTO consents (user_id, type, version) VALUES ($1, 'cloud_voice', 'dev-old')",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    expect_error(
        call(
            &app,
            Method::POST,
            TTS_PATH,
            Some(token),
            "application/json",
            greeting.clone(),
        )
        .await,
        StatusCode::FORBIDDEN,
        "voice_consent_required",
    )
    .await;
    consent(&app, token, true).await;
    expect_no_dispatch(
        &state,
        &auth,
        Charge {
            cost_microusd: 1,
            ..free
        },
        StatusCode::SERVICE_UNAVAILABLE,
        "service_budget_exhausted",
    )
    .await;

    expect_error(
        call(
            &app,
            Method::POST,
            STT_PATH,
            Some(token),
            "audio/ogg",
            clip.clone(),
        )
        .await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported_media_type",
    )
    .await;
    expect_error(
        call(
            &app,
            Method::POST,
            STT_PATH,
            Some(token),
            AUDIO_TYPE,
            "bad WAV",
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_audio",
    )
    .await;
    expect_error(
        call(
            &app,
            Method::POST,
            STT_PATH,
            Some(token),
            AUDIO_TYPE,
            vec![0; wave::MAX_WAV_BYTES + 1],
        )
        .await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
    )
    .await;
    expect_error(
        call(
            &app,
            Method::POST,
            "/v1/voice/stt?language=en&script=cyrl",
            Some(token),
            AUDIO_TYPE,
            clip.clone(),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_locale",
    )
    .await;
    expect_error(
        call(
            &app,
            Method::POST,
            TTS_PATH,
            Some(token),
            "application/json",
            json!({"text":"x".repeat(501)}).to_string(),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_text",
    )
    .await;

    for (language, script, text, transcript) in [
        ("uz", "latn", "Salom", "Salom, Mobi."),
        ("uz", "cyrl", "Салом", "Салом, Моби."),
        ("ru", "cyrl", "Привет", "Привет, Моби."),
        ("en", "latn", "Hello", "Hello, Moby."),
    ] {
        let path = format!("{STT_PATH}?language={language}&script={script}");
        let response = call(
            &app,
            Method::POST,
            &path,
            Some(token),
            AUDIO_TYPE,
            clip.clone(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await,
            json!({
                "text":transcript, "language":language, "script":script, "confidence":1.0
            })
        );
        let response = call(
            &app,
            Method::POST,
            TTS_PATH,
            Some(token),
            "application/json",
            json!({"text":text,"language":language,"script":script}).to_string(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], AUDIO_TYPE);
        let audio = to_bytes(response.into_body(), wave::MAX_WAV_BYTES)
            .await
            .unwrap();
        assert!(!wave::pcm(&audio).unwrap().is_empty());
    }
    let response = call(
        &app,
        Method::GET,
        "/v1/usage",
        Some(token),
        "application/json",
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let quotas = json_body(response).await;
    assert_eq!(quotas["stt_seconds_remaining"], 0);
    assert_eq!(quotas["tts_chars_remaining"], 0);
    assert_eq!(quotas["llm_tokens_remaining"], limits.daily_tokens);
    expect_error(
        call(
            &app,
            Method::POST,
            STT_PATH,
            Some(token),
            AUDIO_TYPE,
            clip.clone(),
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS,
        "quota_exceeded",
    )
    .await;
    expect_error(
        call(
            &app,
            Method::POST,
            TTS_PATH,
            Some(token),
            "application/json",
            greeting.clone(),
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS,
        "quota_exceeded",
    )
    .await;
    expect_no_dispatch(
        &state,
        &auth,
        free,
        StatusCode::TOO_MANY_REQUESTS,
        "quota_exceeded",
    )
    .await;

    consent(&app, token, false).await;
    expect_error(
        call(&app, Method::POST, STT_PATH, Some(token), AUDIO_TYPE, clip).await,
        StatusCode::FORBIDDEN,
        "voice_consent_required",
    )
    .await;
    expect_error(
        call(
            &app,
            Method::POST,
            TTS_PATH,
            Some(token),
            "application/json",
            greeting,
        )
        .await,
        StatusCode::FORBIDDEN,
        "voice_consent_required",
    )
    .await;

    // A withdrawal already holding the row wins over a concurrent provider dispatch.
    consent(&app, token, true).await;
    let mut withdrawal = pool.begin().await.unwrap();
    sqlx::query("UPDATE consents SET revoked_at = now() WHERE user_id = $1 AND type = 'cloud_voice' AND version = 'dev-1'")
        .bind(user_id).execute(&mut *withdrawal).await.unwrap();
    auth.request_id = Uuid::new_v4().to_string();
    let pending = expect_no_dispatch(
        &state,
        &auth,
        free,
        StatusCode::FORBIDDEN,
        "voice_consent_required",
    );
    tokio::pin!(pending);
    assert!(
        timeout(Duration::from_millis(60), &mut pending)
            .await
            .is_err()
    );
    withdrawal.commit().await.unwrap();
    pending.await;

    let settled: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT meter, COUNT(*), SUM(actual_units)::bigint FROM usage_reservations
         WHERE user_id = $1 AND actual_cost_microusd = 0 GROUP BY meter ORDER BY meter",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(settled, [("stt".into(), 4, 4), ("tts".into(), 4, 21)]);
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
}
