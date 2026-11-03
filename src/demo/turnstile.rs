use std::{net::IpAddr, sync::Arc, time::Duration};

use hmac::{Hmac, Mac};
use reqwest::{Client, header, redirect::Policy};
use serde::Deserialize;
use sha2::Sha256;
use url::{Host, Url, form_urlencoded};

const SITEVERIFY: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const ACTION: &str = "demo_chat";
const MAX_REPLY_BYTES: usize = 8 * 1024;

#[derive(Clone)]
pub(super) struct Turnstile(Arc<Settings>);

struct Settings {
    client: Client,
    endpoint: Url,
    secret: String,
    hostname: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum VerifyError {
    Rejected,
    Unavailable,
}

#[derive(Deserialize)]
struct SiteVerifyReply {
    success: bool,
    hostname: Option<String>,
    action: Option<String>,
    #[serde(default, rename = "error-codes")]
    errors: Vec<String>,
}

impl Turnstile {
    pub(super) fn new(secret: String, hostname: &str) -> Result<Self, &'static str> {
        if secret.len() > 512
            || secret.is_empty()
            || !secret.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err("invalid MOBY_TURNSTILE_SECRET_KEY");
        }
        let hostname = match Host::parse(hostname) {
            Ok(Host::Domain(hostname)) if valid_hostname(&hostname) => hostname,
            _ => return Err("MOBY_DEMO_HOSTNAME must be a hostname without a port or path"),
        };
        Self::with_endpoint(
            secret,
            hostname,
            Url::parse(SITEVERIFY).expect("static Siteverify URL"),
            true,
        )
    }

    pub(super) fn with_endpoint(
        secret: String,
        hostname: String,
        endpoint: Url,
        https_only: bool,
    ) -> Result<Self, &'static str> {
        let client = Client::builder()
            .https_only(https_only)
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(4))
            .build()
            .map_err(|_| "could not initialize Turnstile HTTP client")?;
        Ok(Self(Arc::new(Settings {
            client,
            endpoint,
            secret,
            hostname,
        })))
    }

    pub(super) fn rate_bucket(&self, ip: IpAddr) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.0.secret.as_bytes()).expect("valid HMAC key");
        mac.update(b"moby-demo-ip-v1\0");
        match ip.to_canonical() {
            IpAddr::V4(ip) => mac.update(&ip.octets()),
            IpAddr::V6(ip) => mac.update(&ip.octets()[..8]),
        }
        format!("demo:ip:{}", hex::encode(mac.finalize().into_bytes()))
    }

    pub(super) async fn verify(&self, token: &str) -> Result<(), VerifyError> {
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("secret", &self.0.secret)
            .append_pair("response", token)
            .finish();
        let mut response = self
            .0
            .client
            .post(self.0.endpoint.clone())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|_| VerifyError::Unavailable)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|length| length > MAX_REPLY_BYTES as u64)
        {
            return Err(VerifyError::Unavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| VerifyError::Unavailable)?
        {
            if chunk.len() > MAX_REPLY_BYTES - bytes.len() {
                return Err(VerifyError::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        let reply: SiteVerifyReply =
            serde_json::from_slice(&bytes).map_err(|_| VerifyError::Unavailable)?;
        if reply.errors.iter().any(|error| {
            matches!(
                error.as_str(),
                "missing-input-secret" | "invalid-input-secret" | "internal-error"
            )
        }) {
            return Err(VerifyError::Unavailable);
        }
        if reply.success
            && reply
                .hostname
                .as_deref()
                .is_some_and(|host| host.eq_ignore_ascii_case(&self.0.hostname))
            && reply.action.as_deref() == Some(ACTION)
        {
            Ok(())
        } else {
            Err(VerifyError::Rejected)
        }
    }
}

fn valid_hostname(hostname: &str) -> bool {
    hostname.len() <= 253
        && hostname.split('.').all(|label| {
            let bytes = label.as_bytes();
            (1..=63).contains(&bytes.len())
                && bytes[0].is_ascii_alphanumeric()
                && bytes[bytes.len() - 1].is_ascii_alphanumeric()
                && bytes
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        })
}
