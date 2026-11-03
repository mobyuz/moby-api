use std::{env, net::SocketAddr};

use lettre::Address;
use sqlx::{ConnectOptions, postgres::PgConnectOptions};
use url::Url;

use crate::{AccessKeys, Google};

pub struct Config {
    pub bind_addr: SocketAddr,
    pub database: PgConnectOptions,
    pub accounts: AccountConfig,
    pub access_keys: AccessKeys,
    pub google: Option<Google>,
}

#[derive(Clone)]
pub struct AccountConfig {
    pub(crate) smtp_host: String,
    pub(crate) smtp_port: u16,
    pub(crate) mail_from: Address,
    pub(crate) base_url: Url,
    pub(crate) terms_version: String,
    pub(crate) privacy_policy_version: String,
    pub(crate) cloud_voice_version: String,
}

impl Default for AccountConfig {
    fn default() -> Self {
        Self {
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            mail_from: "noreply@moby.test".parse().expect("valid local sender"),
            base_url: Url::parse("http://127.0.0.1:8080/").expect("valid local origin"),
            terms_version: "dev-1".into(),
            privacy_policy_version: "dev-1".into(),
            cloud_voice_version: "dev-1".into(),
        }
    }
}

impl AccountConfig {
    fn from_env() -> Result<Self, &'static str> {
        let defaults = Self::default();
        let smtp_host = env_or("MOBY_DEV_SMTP_HOST", &defaults.smtp_host)?;
        if smtp_host != "mailpit"
            && !smtp_host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        {
            return Err("MOBY_DEV_SMTP_HOST must point to Mailpit or a loopback IP");
        }
        let smtp_port = env_or("MOBY_DEV_SMTP_PORT", "1025")?
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or("MOBY_DEV_SMTP_PORT must be a valid TCP port")?;
        let mail_from = env_or("MOBY_MAIL_FROM", defaults.mail_from.as_ref())?
            .parse()
            .map_err(|_| "MOBY_MAIL_FROM must be an email address")?;
        let base_url = Url::parse(&env_or(
            "MOBY_ACCOUNT_BASE_URL",
            defaults.base_url.as_str(),
        )?)
        .map_err(|_| "MOBY_ACCOUNT_BASE_URL must be a valid origin")?;
        let local = match base_url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain("localhost")) => true,
            _ => false,
        };
        if !(base_url.scheme() == "https" || (local && base_url.scheme() == "http"))
            || base_url.host().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || base_url.path() != "/"
        {
            return Err("MOBY_ACCOUNT_BASE_URL must be an HTTPS origin or a local HTTP origin");
        }
        Ok(Self {
            smtp_host,
            smtp_port,
            mail_from,
            base_url,
            terms_version: policy_version("MOBY_TERMS_VERSION", &defaults.terms_version)?,
            privacy_policy_version: policy_version(
                "MOBY_PRIVACY_POLICY_VERSION",
                &defaults.privacy_policy_version,
            )?,
            cloud_voice_version: policy_version(
                "MOBY_CLOUD_VOICE_VERSION",
                &defaults.cloud_voice_version,
            )?,
        })
    }
}

fn env_or(name: &'static str, default: &str) -> Result<String, &'static str> {
    match env::var(name) {
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => Ok(default.into()),
        Err(env::VarError::NotUnicode(_)) => Err(name),
    }
}

fn policy_version(name: &'static str, default: &str) -> Result<String, &'static str> {
    let value = env_or(name, default)?;
    if !valid_policy_version(&value) {
        return Err(
            "policy versions must contain 1–64 ASCII letters, digits, dots, underscores or hyphens",
        );
    }
    Ok(value)
}

pub(crate) fn valid_policy_version(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

impl Config {
    pub fn from_env() -> Result<Self, &'static str> {
        let bind_addr = match env::var("MOBY_BIND_ADDR") {
            Ok(value) => value
                .parse()
                .map_err(|_| "MOBY_BIND_ADDR must be a socket address")?,
            Err(env::VarError::NotPresent) => SocketAddr::from(([127, 0, 0, 1], 8080)),
            Err(env::VarError::NotUnicode(_)) => return Err("MOBY_BIND_ADDR must be Unicode"),
        };

        let database_url = env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
        if !database_url.starts_with("postgres://") && !database_url.starts_with("postgresql://") {
            return Err("DATABASE_URL must be a PostgreSQL URL");
        }
        let database = database_url
            .parse::<PgConnectOptions>()
            .map_err(|_| "DATABASE_URL must be a valid PostgreSQL URL")?
            .disable_statement_logging();

        Ok(Self {
            bind_addr,
            database,
            accounts: AccountConfig::from_env()?,
            access_keys: AccessKeys::from_env(bind_addr.ip().is_loopback())?,
            google: Google::from_env()?,
        })
    }
}
