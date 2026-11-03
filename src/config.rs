use std::{env, net::SocketAddr};

use lettre::Address;
use sqlx::{ConnectOptions, postgres::PgConnectOptions};
use url::Url;

use crate::{AccessKeys, Demo, Google, UsageConfig, usage::DemoBudget};

pub struct Config {
    pub bind_addr: SocketAddr,
    pub database: PgConnectOptions,
    pub accounts: AccountConfig,
    pub access_keys: AccessKeys,
    pub google: Option<Google>,
    pub usage: UsageConfig,
    pub demo: Option<Demo>,
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

impl UsageConfig {
    fn from_env() -> Result<Self, &'static str> {
        let defaults = Self::default();
        let agent_enabled = env_or("MOBY_AGENT_ENABLED", "true")?
            .parse()
            .map_err(|_| "MOBY_AGENT_ENABLED must be true or false")?;
        let voice_enabled = env_or("MOBY_VOICE_ENABLED", "false")?
            .parse()
            .map_err(|_| "MOBY_VOICE_ENABLED must be true or false")?;
        Ok(Self {
            agent_enabled,
            voice_enabled,
            daily_tokens: quota("MOBY_LLM_DAILY_TOKENS", defaults.daily_tokens)?,
            daily_stt_seconds: quota("MOBY_STT_DAILY_SECONDS", defaults.daily_stt_seconds)?,
            daily_tts_chars: quota("MOBY_TTS_DAILY_CHARS", defaults.daily_tts_chars)?,
            daily_budget_microusd: budget("MOBY_LLM_DAILY_BUDGET_MICROUSD")?,
            monthly_budget_microusd: budget("MOBY_LLM_MONTHLY_BUDGET_MICROUSD")?,
        })
    }
}

fn quota(name: &'static str, default: u32) -> Result<u32, &'static str> {
    env_or(name, &default.to_string())?
        .parse()
        .map_err(|_| name)
}

fn budget(name: &'static str) -> Result<i64, &'static str> {
    env_or(name, "0")?
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .ok_or(name)
}

fn demo_from_env() -> Result<Option<Demo>, &'static str> {
    let enabled: bool = env_or("MOBY_DEMO_ENABLED", "false")?
        .parse()
        .map_err(|_| "MOBY_DEMO_ENABLED must be true or false")?;
    if !enabled {
        return Ok(None);
    }
    let secret = env::var("MOBY_TURNSTILE_SECRET_KEY")
        .map_err(|_| "MOBY_TURNSTILE_SECRET_KEY is required")?;
    let hostname = env::var("MOBY_DEMO_HOSTNAME").map_err(|_| "MOBY_DEMO_HOSTNAME is required")?;
    let ip_limit = quota("MOBY_DEMO_REQUESTS_PER_15_MINUTES", 12)?;
    if !(1..=1000).contains(&ip_limit) {
        return Err("MOBY_DEMO_REQUESTS_PER_15_MINUTES must be between 1 and 1000");
    }
    let budget = DemoBudget {
        daily_units: quota("MOBY_DEMO_DAILY_TOKENS", 60_000)?,
        daily_cost_microusd: budget("MOBY_DEMO_DAILY_BUDGET_MICROUSD")?,
        monthly_cost_microusd: budget("MOBY_DEMO_MONTHLY_BUDGET_MICROUSD")?,
    };
    Demo::new(secret, &hostname, budget, ip_limit as i32).map(Some)
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
            usage: UsageConfig::from_env()?,
            demo: demo_from_env()?,
        })
    }
}
