use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use axum::{
    Form, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::{Client, Method, Response as HttpResponse, redirect::Policy};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tokio::{net::TcpListener, sync::Mutex};
use url::Url;
use uuid::Uuid;

use super::{Google, provider::pkce_challenge};
use crate::{AccessKeys, MIGRATOR, config::AccountConfig, router};

const CLIENT_ID: &str = "moby-mock.apps.googleusercontent.com";
const DESKTOP_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const PASSWORD: &str = "Synthetic password for Google link";

struct Grant {
    challenge: String,
    nonce: String,
    email: String,
    subject: String,
    fault: Fault,
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    Nonce,
    Audience,
    Unverified,
    Signature,
}

struct Mock {
    grant: Mutex<Option<Grant>>,
    signing: EncodingKey,
    jwks: Value,
    callback: String,
}

async fn token(
    State(mock): State<Arc<Mock>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(grant) = mock.grant.lock().await.take() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let valid = form.get("code").is_some_and(|code| code == "provider-code")
        && form
            .get("grant_type")
            .is_some_and(|value| value == "authorization_code")
        && form.get("client_id").is_some_and(|id| id == CLIENT_ID)
        && form
            .get("client_secret")
            .is_some_and(|secret| secret == "moby-mock-client-secret")
        && form.get("redirect_uri") == Some(&mock.callback)
        && form
            .get("code_verifier")
            .is_some_and(|verifier| pkce_challenge(verifier) == grant.challenge);
    if !valid {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("mock-rsa".into());
    let mut signed = jsonwebtoken::encode(&header, &json!({
        "iss": "https://accounts.google.com",
        "aud": if matches!(grant.fault, Fault::Audience) { "unexpected-client" } else { CLIENT_ID },
        "exp": now + 120,
        "iat": now,
        "nonce": if matches!(grant.fault, Fault::Nonce) { "wrong-nonce".into() } else { grant.nonce },
        "sub": grant.subject,
        "email": grant.email,
        "email_verified": !matches!(grant.fault, Fault::Unverified)
    }), &mock.signing).unwrap();
    if matches!(grant.fault, Fault::Signature) {
        let offset = signed.rfind('.').unwrap() + 1;
        let first = if signed.as_bytes()[offset] == b'A' {
            "B"
        } else {
            "A"
        };
        signed.replace_range(offset..=offset, first);
    }
    Json(json!({"id_token": signed})).into_response()
}

async fn jwks(State(mock): State<Arc<Mock>>) -> Json<Value> {
    Json(mock.jwks.clone())
}

struct TestApi {
    http: Client,
    api: Url,
    mock: Arc<Mock>,
}

impl TestApi {
    async fn request(
        &self,
        method: Method,
        path: &str,
        expected: u16,
        body: Option<Value>,
        bearer: Option<&str>,
    ) -> HttpResponse {
        let mut request = self
            .http
            .request(method.clone(), self.api.join(path).unwrap());
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body.to_string());
        }
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        let response = request.send().await.unwrap();
        assert_eq!(
            response.status().as_u16(),
            expected,
            "{method} {}",
            path.split('?').next().unwrap()
        );
        response
    }

    async fn google_callback(&self, subject: &str, email: &str, fault: Fault) -> Url {
        let desktop_state = URL_SAFE_NO_PAD.encode([8u8; 32]);
        let challenge = pkce_challenge(DESKTOP_VERIFIER);
        let path = format!(
            "/v1/auth/google/start?desktop_port=49152&desktop_state={desktop_state}&code_challenge={challenge}&terms_version=dev-1&privacy_policy_version=dev-1"
        );
        let start = self.request(Method::GET, &path, 303, None, None).await;
        assert_eq!(start.headers()["referrer-policy"], "no-referrer");
        let authorize = location(&start);
        assert_eq!(authorize.path(), "/authorize");
        assert_eq!(
            query(&authorize, "code_challenge_method").as_deref(),
            Some("S256")
        );
        assert_eq!(query(&authorize, "scope").as_deref(), Some("openid email"));
        let state = query(&authorize, "state").unwrap();
        let nonce = query(&authorize, "nonce").unwrap();
        let provider_challenge = query(&authorize, "code_challenge").unwrap();
        assert_ne!(provider_challenge, challenge);
        *self.mock.grant.lock().await = Some(Grant {
            challenge: provider_challenge,
            nonce,
            email: email.into(),
            subject: subject.into(),
            fault,
        });

        let wrong = "00".repeat(32);
        let wrong_state = format!("/v1/auth/google/callback?state={wrong}&code=provider-code");
        self.request(Method::GET, &wrong_state, 400, None, None)
            .await;
        let callback = format!("/v1/auth/google/callback?state={state}&code=provider-code");
        let response = self.request(Method::GET, &callback, 303, None, None).await;
        let redirect = location(&response);
        assert_eq!(redirect.host_str(), Some("127.0.0.1"));
        assert_eq!(redirect.port(), Some(49152));
        assert_eq!(redirect.path(), "/oauth/moby");
        assert_eq!(query(&redirect, "state"), Some(desktop_state));
        self.request(Method::GET, &callback, 400, None, None).await;
        redirect
    }

    async fn proof(&self, subject: &str, email: &str) -> Value {
        let redirect = self.google_callback(subject, email, Fault::None).await;
        assert!(query(&redirect, "error").is_none());
        json!({"code": query(&redirect, "code").unwrap(), "code_verifier": DESKTOP_VERIFIER})
    }
}

fn location(response: &HttpResponse) -> Url {
    Url::parse(response.headers()["location"].to_str().unwrap()).unwrap()
}

fn query(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

async fn body(response: HttpResponse) -> Value {
    serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
}

#[test]
fn pkce_matches_the_rfc7636_s256_vector() {
    assert_eq!(
        pkce_challenge(DESKTOP_VERIFIER),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[tokio::test]
#[ignore = "requires an isolated PostgreSQL database in MOBY_GOOGLE_TEST_DATABASE_URL"]
async fn google_round_trip() {
    let database = std::env::var("MOBY_GOOGLE_TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database)
        .await
        .unwrap();
    MIGRATOR.run(&pool).await.unwrap();

    let mock_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = Url::parse(&format!("http://{}/", mock_listener.local_addr().unwrap())).unwrap();
    let api = Url::parse(&format!("http://{}/", api_listener.local_addr().unwrap())).unwrap();
    let rsa = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
    let der = rsa.to_pkcs1_der().unwrap();
    let mock = Arc::new(Mock {
        grant: Mutex::new(None),
        signing: EncodingKey::from_rsa_der(der.as_bytes()),
        jwks: json!({"keys": [{
            "kty": "RSA", "alg": "RS256", "kid": "mock-rsa", "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(rsa.n().to_bytes_be()),
            "e": URL_SAFE_NO_PAD.encode(rsa.e().to_bytes_be())
        }]}),
        callback: api.join("/v1/auth/google/callback").unwrap().to_string(),
    });
    let mock_app = Router::new()
        .route("/token", post(token))
        .route("/jwks", get(jwks))
        .with_state(mock.clone());
    let app = router(
        pool.clone(),
        AccountConfig::default(),
        AccessKeys::ephemeral().unwrap(),
        Some(Google::mock(&origin, &api)),
    );
    let mock_task = tokio::spawn(async move {
        axum::serve(mock_listener, mock_app).await.unwrap();
    });
    let api_task = tokio::spawn(async move {
        axum::serve(
            api_listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let http = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let test = TestApi { http, api, mock };
    let suffix = Uuid::new_v4().simple();
    let email = format!("google-{suffix}@example.test");
    let subject = format!("google-sub-{suffix}");

    for fault in [
        Fault::Nonce,
        Fault::Audience,
        Fault::Unverified,
        Fault::Signature,
    ] {
        let redirect = test.google_callback(&subject, &email, fault).await;
        assert_eq!(
            query(&redirect, "error").as_deref(),
            Some("authorization_failed")
        );
        assert!(query(&redirect, "code").is_none());
    }

    let expired = test.proof(&subject, &email).await;
    sqlx::query(
        "UPDATE google_handoffs SET expires_at = now() - interval '1 second' WHERE subject = $1",
    )
    .bind(&subject)
    .execute(&pool)
    .await
    .unwrap();
    test.request(
        Method::POST,
        "/v1/auth/google/exchange",
        400,
        Some(expired),
        None,
    )
    .await;

    let proof = test.proof(&subject, &email).await;
    let mut wrong = proof.clone();
    wrong["code_verifier"] = json!("x".repeat(43));
    test.request(
        Method::POST,
        "/v1/auth/google/exchange",
        400,
        Some(wrong),
        None,
    )
    .await;
    let first = body(
        test.request(
            Method::POST,
            "/v1/auth/google/exchange",
            200,
            Some(proof.clone()),
            None,
        )
        .await,
    )
    .await;
    test.request(
        Method::POST,
        "/v1/auth/google/exchange",
        400,
        Some(proof),
        None,
    )
    .await;
    let access = first["access_token"].as_str().unwrap();
    let me = body(
        test.request(Method::GET, "/v1/me", 200, None, Some(access))
            .await,
    )
    .await;
    assert_eq!(me["email"], email);
    let export = body(
        test.request(Method::POST, "/v1/me/export", 200, None, Some(access))
            .await,
    )
    .await;
    assert_eq!(
        export["identities"],
        json!([{"provider": "google", "subject": subject}])
    );
    assert_eq!(export["consents"].as_array().unwrap().len(), 2);

    let local_email = format!("password-{suffix}@example.test");
    let local_subject = format!("linked-sub-{suffix}");
    let local_id = Uuid::new_v4();
    let salt = SaltString::encode_b64(b"local-test-salt!").unwrap();
    let hash = Argon2::default()
        .hash_password(PASSWORD.as_bytes(), &salt)
        .unwrap()
        .to_string();
    sqlx::query("INSERT INTO users (id, email, password_hash, email_verified_at) VALUES ($1, $2, $3, now())")
        .bind(local_id).bind(&local_email).bind(&hash).execute(&pool).await.unwrap();
    let collision = test.proof(&local_subject, &local_email).await;
    let error = body(
        test.request(
            Method::POST,
            "/v1/auth/google/exchange",
            409,
            Some(collision.clone()),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(error["code"], "account_link_required");
    let password_login = body(
        test.request(
            Method::POST,
            "/v1/auth/login",
            200,
            Some(json!({"email": local_email, "password": PASSWORD})),
            None,
        )
        .await,
    )
    .await;
    let local_access = password_login["access_token"].as_str().unwrap();
    let mut link = collision;
    link["password"] = json!("Incorrect password used in the test");
    test.request(
        Method::POST,
        "/v1/auth/google/link",
        401,
        Some(link.clone()),
        Some(local_access),
    )
    .await;
    link["password"] = json!(PASSWORD);
    test.request(
        Method::POST,
        "/v1/auth/google/link",
        204,
        Some(link),
        Some(local_access),
    )
    .await;
    let linked = test.proof(&local_subject, &local_email).await;
    let signed_in = body(
        test.request(
            Method::POST,
            "/v1/auth/google/exchange",
            200,
            Some(linked),
            None,
        )
        .await,
    )
    .await;
    let signed_access = signed_in["access_token"].as_str().unwrap();
    let linked_me = body(
        test.request(Method::GET, "/v1/me", 200, None, Some(signed_access))
            .await,
    )
    .await;
    assert_eq!(linked_me["id"], local_id.to_string());

    let foreign = test.proof(&local_subject, &local_email).await;
    test.request(Method::DELETE, "/v1/me", 401, Some(foreign), Some(access))
        .await;
    let own = test.proof(&subject, &email).await;
    test.request(Method::DELETE, "/v1/me", 204, Some(own), Some(access))
        .await;
    test.request(Method::GET, "/v1/me", 401, None, Some(access))
        .await;
    test.request(Method::GET, "/v1/me", 200, None, Some(signed_access))
        .await;
    mock_task.abort();
    api_task.abort();
}
