use std::{
    env,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation,
    jwk::{AlgorithmParameters, JwkSet},
};
use reqwest::{Client, Response, header::CACHE_CONTROL, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use url::{Host, Url};

use crate::accounts::{AuthError, normalize_email};

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const CALLBACK_PATH: &str = "/v1/auth/google/callback";
const MAX_PROVIDER_BODY: usize = 64 * 1024;

#[derive(Clone)]
pub struct Google {
    inner: Arc<Provider>,
}

struct Provider {
    client_id: String,
    secret: String,
    redirect: Url,
    authorize: Url,
    token: Url,
    jwks: Url,
    http: Client,
    cache: Mutex<Option<CachedKeys>>,
}

struct CachedKeys {
    keys: JwkSet,
    expires: Instant,
}

pub(super) struct GoogleIdentity {
    pub(super) subject: String,
    pub(super) email: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    aud: Audience,
    azp: Option<String>,
    nonce: String,
    email: String,
    email_verified: bool,
    iat: u64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    Single(String),
    Multiple(Vec<String>),
}

impl Google {
    pub(crate) fn from_env() -> Result<Option<Self>, &'static str> {
        let [id, secret, redirect] = [
            "MOBY_GOOGLE_CLIENT_ID",
            "MOBY_GOOGLE_CLIENT_SECRET",
            "MOBY_GOOGLE_REDIRECT_URI",
        ]
        .map(env::var_os);
        let (id, secret, redirect) = match (id, secret, redirect) {
            (None, None, None) => return Ok(None),
            (Some(id), Some(secret), Some(redirect)) => (id, secret, redirect),
            _ => return Err("set all three Google OAuth settings together"),
        };
        let id = id.into_string().map_err(|_| "invalid Google client ID")?;
        let secret = secret
            .into_string()
            .map_err(|_| "invalid Google client secret")?;
        if !id.ends_with(".apps.googleusercontent.com") || !valid_config_value(&id) {
            return Err("invalid Google client ID");
        }
        if !valid_config_value(&secret) {
            return Err("invalid Google client secret");
        }
        let redirect = redirect
            .to_str()
            .and_then(|value| Url::parse(value).ok())
            .filter(valid_redirect)
            .ok_or("invalid Google redirect URI")?;
        Ok(Some(Self::new(
            id,
            secret,
            redirect,
            [
                Url::parse(AUTHORIZE_URL).expect("static authorize URL"),
                Url::parse(TOKEN_URL).expect("static token URL"),
                Url::parse(JWKS_URL).expect("static JWKS URL"),
            ],
            true,
        )?))
    }

    fn new(
        client_id: String,
        secret: String,
        redirect: Url,
        endpoints: [Url; 3],
        https_only: bool,
    ) -> Result<Self, &'static str> {
        let [authorize, token, jwks] = endpoints;
        let http = Client::builder()
            .https_only(https_only)
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| "could not initialize Google HTTP client")?;
        Ok(Self {
            inner: Arc::new(Provider {
                client_id,
                secret,
                redirect,
                authorize,
                token,
                jwks,
                http,
                cache: Mutex::new(None),
            }),
        })
    }

    #[cfg(test)]
    pub(super) fn mock(origin: &Url, api: &Url) -> Self {
        Self::new(
            "moby-mock.apps.googleusercontent.com".into(),
            "moby-mock-client-secret".into(),
            api.join(CALLBACK_PATH).unwrap(),
            [
                origin.join("/authorize").unwrap(),
                origin.join("/token").unwrap(),
                origin.join("/jwks").unwrap(),
            ],
            false,
        )
        .unwrap()
    }

    pub(super) fn authorization_url(&self, state: &str) -> Url {
        let mut url = self.inner.authorize.clone();
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.inner.client_id.as_str()),
            ("redirect_uri", self.inner.redirect.as_str()),
            ("response_type", "code"),
            ("scope", "openid email"),
            ("prompt", "select_account"),
            ("state", state),
            ("nonce", &self.bound_value(b"nonce", state)),
            (
                "code_challenge",
                &pkce_challenge(&self.bound_value(b"pkce", state)),
            ),
            ("code_challenge_method", "S256"),
        ]);
        url
    }

    fn bound_value(&self, label: &[u8], state: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.inner.secret.as_bytes())
            .expect("HMAC accepts a key of any length");
        mac.update(b"moby/google/oidc/v1/");
        mac.update(label);
        mac.update(b"/");
        mac.update(state.as_bytes());
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    }

    pub(super) async fn exchange(
        &self,
        state: &str,
        code: &str,
    ) -> Result<GoogleIdentity, AuthError> {
        let verifier = self.bound_value(b"pkce", state);
        let response = self
            .inner
            .http
            .post(self.inner.token.clone())
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", self.inner.client_id.as_str()),
                ("client_secret", self.inner.secret.as_str()),
                ("redirect_uri", self.inner.redirect.as_str()),
                ("code_verifier", verifier.as_str()),
                ("code", code),
            ])
            .send()
            .await
            .map_err(|_| AuthError::ProviderUnavailable)?;
        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            return Err(AuthError::Unauthorized);
        }
        let token: TokenResponse = bounded_json(response).await?;
        self.identity(&token.id_token, &self.bound_value(b"nonce", state))
            .await
    }

    async fn identity(&self, token: &str, nonce: &str) -> Result<GoogleIdentity, AuthError> {
        if token.len() > 8192 {
            return Err(AuthError::Unauthorized);
        }
        let header = jsonwebtoken::decode_header(token).map_err(|_| AuthError::Unauthorized)?;
        let kid = header
            .kid
            .filter(|kid| !kid.is_empty() && kid.len() <= 128)
            .ok_or(AuthError::Unauthorized)?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::Unauthorized);
        }
        let key = self.verification_key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["https://accounts.google.com", "accounts.google.com"]);
        validation.set_audience(&[&self.inner.client_id]);
        validation.set_required_spec_claims(&["exp", "sub", "iss", "aud"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let claims = jsonwebtoken::decode::<Claims>(token, &key, &validation)
            .map_err(|_| AuthError::Unauthorized)?
            .claims;
        let client_id = &self.inner.client_id;
        let audience_matches = match &claims.aud {
            Audience::Single(audience) => audience == client_id,
            Audience::Multiple(audiences) => {
                audiences.iter().any(|aud| aud == client_id)
                    && (audiences.len() == 1 || claims.azp.as_ref() == Some(client_id))
            }
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AuthError::Unavailable)?
            .as_secs();
        if !audience_matches
            || claims.azp.as_ref().is_some_and(|azp| azp != client_id)
            || claims.nonce != nonce
            || !claims.email_verified
            || claims.sub.is_empty()
            || claims.sub.len() > 255
            || !claims.sub.bytes().all(|byte| byte.is_ascii_graphic())
            || claims.iat > now + 30
            || now.saturating_sub(claims.iat) > 600
        {
            return Err(AuthError::Unauthorized);
        }
        Ok(GoogleIdentity {
            subject: claims.sub,
            email: normalize_email(&claims.email).map_err(|_| AuthError::Unauthorized)?,
        })
    }

    async fn verification_key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        let mut cache = self.inner.cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.expires > Instant::now())
            && let Some(key) = cached.keys.find(kid)
        {
            return rsa_key(key);
        }
        let response = self
            .inner
            .http
            .get(self.inner.jwks.clone())
            .send()
            .await
            .map_err(|_| AuthError::ProviderUnavailable)?;
        let ttl = response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|header| header.to_str().ok())
            .and_then(|value| {
                value
                    .split(',')
                    .find_map(|item| item.trim().strip_prefix("max-age=")?.parse::<u64>().ok())
            })
            .unwrap_or(300)
            .min(3600);
        let keys: JwkSet = bounded_json(response).await?;
        let key = keys
            .find(kid)
            .ok_or(AuthError::Unauthorized)
            .and_then(rsa_key);
        *cache = Some(CachedKeys {
            keys,
            expires: Instant::now() + Duration::from_secs(ttl),
        });
        key
    }
}

fn rsa_key(key: &jsonwebtoken::jwk::Jwk) -> Result<DecodingKey, AuthError> {
    if !matches!(key.algorithm, AlgorithmParameters::RSA(_)) {
        return Err(AuthError::Unauthorized);
    }
    DecodingKey::from_jwk(key).map_err(|_| AuthError::Unauthorized)
}

async fn bounded_json<T: DeserializeOwned>(mut response: Response) -> Result<T, AuthError> {
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > MAX_PROVIDER_BODY as u64)
    {
        return Err(AuthError::ProviderUnavailable);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AuthError::ProviderUnavailable)?
    {
        if body.len() + chunk.len() > MAX_PROVIDER_BODY {
            return Err(AuthError::ProviderUnavailable);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| AuthError::ProviderUnavailable)
}

fn valid_config_value(value: &str) -> bool {
    (1..=512).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_redirect(url: &Url) -> bool {
    let loopback = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain("localhost")) => true,
        _ => false,
    };
    (url.scheme() == "https" || (loopback && url.scheme() == "http"))
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == CALLBACK_PATH
}

pub(super) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(super) fn valid_challenge(value: &str) -> bool {
    value.len() == 43
        && URL_SAFE_NO_PAD
            .decode(value)
            .is_ok_and(|bytes| bytes.len() == 32)
}

pub(super) fn valid_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
}
