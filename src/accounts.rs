use std::{net::IpAddr, sync::Arc};

use argon2::{
    Argon2, PasswordHasher, PasswordVerifier,
    password_hash::{PasswordHash, SaltString},
};
use lettre::Address;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::{Semaphore, mpsc};
use uuid::Uuid;

use crate::config::AccountConfig;

mod mail;
pub(crate) mod routes;

pub(crate) const VERIFY_PATH: &str = "/v1/auth/verify-email";
pub(crate) const RESET_PATH: &str = "/v1/auth/password/reset";

#[derive(Clone)]
pub(crate) struct Accounts {
    config: AccountConfig,
    mail: mpsc::Sender<mail::Job>,
    password_workers: Arc<Semaphore>,
}

#[derive(Clone, Copy)]
enum TokenKind {
    VerifyEmail,
    PasswordReset,
}

impl TokenKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::VerifyEmail => "verify_email",
            Self::PasswordReset => "password_reset",
        }
    }
}

pub(crate) enum AuthError {
    InvalidEmail,
    InvalidPassword,
    InvalidConsent,
    InvalidToken,
    InvalidInstallation,
    InvalidProfileName,
    InvalidConsentVersion,
    InvalidOAuth,
    AccountLinkRequired,
    IdentityConflict,
    ProviderUnavailable,
    Unauthorized,
    NotFound,
    RateLimited,
    Unavailable,
}

impl From<sqlx::Error> for AuthError {
    fn from(_: sqlx::Error) -> Self {
        tracing::warn!("account database operation failed");
        Self::Unavailable
    }
}

impl Accounts {
    pub(crate) fn new(pool: PgPool, config: AccountConfig) -> Self {
        let (mail, jobs) = mpsc::channel(32);
        tokio::spawn(mail::run(pool, config.clone(), jobs));
        Self {
            config,
            mail,
            password_workers: Arc::new(Semaphore::new(4)),
        }
    }

    pub(crate) fn config(&self) -> &AccountConfig {
        &self.config
    }

    async fn register(
        &self,
        pool: &PgPool,
        ip: IpAddr,
        email: String,
        password: String,
        terms: String,
        privacy: String,
    ) -> Result<(), AuthError> {
        let email = normalize_email(&email)?;
        validate_password(&password)?;
        if terms != self.config.terms_version || privacy != self.config.privacy_policy_version {
            return Err(AuthError::InvalidConsent);
        }
        check_rate(pool, "register", ip, Some((&email, 5))).await?;
        let permit = self
            .mail
            .try_reserve()
            .map_err(|_| AuthError::Unavailable)?;
        let password_hash = self.hash_password(password).await?;
        let (token, hash) = new_token()?;
        let mut tx = pool.begin().await?;

        sqlx::query!(
            "INSERT INTO users (id, email) VALUES ($1, $2) ON CONFLICT (email) DO NOTHING",
            Uuid::new_v4(),
            email,
        )
        .execute(&mut *tx)
        .await?;

        let user = sqlx::query_scalar!(
            "SELECT id FROM users WHERE email = $1 AND status = 'active'
                AND email_verified_at IS NULL FOR UPDATE",
            email,
        )
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(user_id) = user {
            sqlx::query!(
                "INSERT INTO email_tokens
                    (token_hash, user_id, kind, expires_at, pending_password_hash, terms_version, privacy_policy_version)
                 VALUES ($1, $2, 'verify_email', now() + interval '24 hours', $3, $4, $5)",
                &hash[..], user_id, password_hash, terms, privacy,
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            permit.send(mail::Job::Verify { email, token });
        } else {
            tx.commit().await?;
        }

        Ok(())
    }

    async fn verify_email(
        &self,
        pool: &PgPool,
        ip: IpAddr,
        token: String,
        password: String,
    ) -> Result<(), AuthError> {
        let hash = token_hash(&token)?;
        validate_password(&password)?;
        check_rate(pool, "verify", ip, None).await?;
        let candidate = sqlx::query!(
            r#"SELECT user_id, pending_password_hash AS "password_hash!", terms_version AS "terms_version!",
                      privacy_policy_version AS "privacy_policy_version!"
               FROM email_tokens WHERE token_hash = $1 AND kind = 'verify_email'
                    AND used_at IS NULL AND expires_at > now()"#,
            &hash[..],
        )
        .fetch_optional(pool)
        .await?
        .ok_or(AuthError::InvalidToken)?;

        let password_hash = candidate.password_hash;
        if !self
            .password_matches(password, password_hash.clone())
            .await?
        {
            return Err(AuthError::InvalidToken);
        }

        let user_id = candidate.user_id;
        let mut tx = pool.begin().await?;
        if sqlx::query_scalar!(
            "SELECT id FROM users WHERE id = $1 AND status = 'active'
                AND email_verified_at IS NULL FOR UPDATE",
            user_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .is_none()
        {
            return Err(AuthError::InvalidToken);
        }

        consume(&mut tx, &hash, user_id, TokenKind::VerifyEmail).await?;
        sqlx::query!(
            "UPDATE users SET password_hash = $2, email_verified_at = now() WHERE id = $1",
            user_id,
            password_hash,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "INSERT INTO consents (user_id, type, version)
                VALUES ($1, 'terms', $2), ($1, 'privacy_policy', $3)",
            user_id,
            candidate.terms_version,
            candidate.privacy_policy_version,
        )
        .execute(&mut *tx)
        .await?;
        invalidate_remaining(&mut tx, user_id, TokenKind::VerifyEmail).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn forgot(&self, pool: &PgPool, ip: IpAddr, email: String) -> Result<(), AuthError> {
        let email = normalize_email(&email)?;
        check_rate(pool, "forgot", ip, Some((&email, 3))).await?;
        self.mail
            .try_send(mail::Job::Forgot { email })
            .map_err(|_| AuthError::Unavailable)
    }

    async fn reset(
        &self,
        pool: &PgPool,
        ip: IpAddr,
        token: String,
        password: String,
    ) -> Result<(), AuthError> {
        let hash = token_hash(&token)?;
        validate_password(&password)?;
        check_rate(pool, "reset", ip, None).await?;
        let user_id = sqlx::query_scalar!(
            "SELECT user_id FROM email_tokens WHERE token_hash = $1 AND kind = 'password_reset'
                AND used_at IS NULL AND expires_at > now()",
            &hash[..],
        )
        .fetch_optional(pool)
        .await?
        .ok_or(AuthError::InvalidToken)?;
        let password_hash = self.hash_password(password).await?;

        let mut tx = pool.begin().await?;
        if lock_verified(&mut tx, user_id).await?.is_none() {
            return Err(AuthError::InvalidToken);
        }
        consume(&mut tx, &hash, user_id, TokenKind::PasswordReset).await?;
        sqlx::query!(
            "UPDATE users SET password_hash = $2 WHERE id = $1",
            user_id,
            password_hash
        )
        .execute(&mut *tx)
        .await?;
        invalidate_remaining(&mut tx, user_id, TokenKind::PasswordReset).await?;
        crate::sessions::revoke_user_sessions(&mut tx, user_id).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn hash_password(&self, password: String) -> Result<String, AuthError> {
        self.password_job(move || {
            let mut salt = [0u8; 16];
            getrandom::fill(&mut salt).map_err(|_| AuthError::Unavailable)?;
            let salt = SaltString::encode_b64(&salt).map_err(|_| AuthError::Unavailable)?;
            Argon2::default()
                .hash_password(password.as_bytes(), &salt)
                .map(|hash| hash.to_string())
                .map_err(|_| AuthError::Unavailable)
        })
        .await
    }

    pub(crate) async fn password_matches(
        &self,
        password: String,
        hash: String,
    ) -> Result<bool, AuthError> {
        self.password_job(move || {
            let parsed = PasswordHash::new(&hash).map_err(|_| AuthError::Unavailable)?;
            Ok(Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        })
        .await
    }

    async fn password_job<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> Result<T, AuthError> + Send + 'static,
    ) -> Result<T, AuthError> {
        let permit = self
            .password_workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| AuthError::Unavailable)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            job()
        })
        .await
        .map_err(|_| AuthError::Unavailable)?
    }
}

async fn lock_verified(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT id FROM users WHERE id = $1 AND status = 'active' AND email_verified_at IS NOT NULL FOR UPDATE",
        user_id,
    ).fetch_optional(&mut **tx).await
}

async fn consume(
    tx: &mut Transaction<'_, Postgres>,
    hash: &[u8; 32],
    user_id: Uuid,
    kind: TokenKind,
) -> Result<(), AuthError> {
    let changed = sqlx::query!(
        "UPDATE email_tokens SET used_at = now(), pending_password_hash = NULL,
            terms_version = NULL, privacy_policy_version = NULL
         WHERE token_hash = $1 AND user_id = $2 AND kind = $3
            AND used_at IS NULL AND expires_at > clock_timestamp()",
        &hash[..],
        user_id,
        kind.as_str(),
    )
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(AuthError::InvalidToken);
    }
    Ok(())
}

async fn invalidate_remaining(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    kind: TokenKind,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM email_tokens WHERE user_id = $1 AND kind = $2 AND used_at IS NULL",
        user_id,
        kind.as_str()
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) fn normalize_email(email: &str) -> Result<String, AuthError> {
    let email = email.trim().to_ascii_lowercase();
    if email.len() > 254 || !email.is_ascii() || email.parse::<Address>().is_err() {
        return Err(AuthError::InvalidEmail);
    }
    Ok(email)
}

pub(crate) fn validate_password(password: &str) -> Result<(), AuthError> {
    if !(15..=128).contains(&password.chars().count()) {
        return Err(AuthError::InvalidPassword);
    }
    Ok(())
}

pub(crate) fn new_token() -> Result<(String, [u8; 32]), AuthError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| AuthError::Unavailable)?;
    Ok((hex::encode(bytes), Sha256::digest(bytes).into()))
}

pub(crate) fn token_hash(token: &str) -> Result<[u8; 32], AuthError> {
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(token, &mut bytes).map_err(|_| AuthError::InvalidToken)?;
    Ok(Sha256::digest(bytes).into())
}

pub(crate) async fn check_rate(
    pool: &PgPool,
    scope: &str,
    ip: IpAddr,
    email: Option<(&str, i32)>,
) -> Result<(), AuthError> {
    hit_bucket(pool, &format!("{scope}:ip:{ip}"), 40).await?;
    if let Some((email, limit)) = email {
        hit_bucket(pool, &email_bucket(scope, email), limit).await?;
    }
    Ok(())
}

fn email_bucket(scope: &str, email: &str) -> String {
    let id = hex::encode(Sha256::digest(email.as_bytes()));
    format!("{scope}:email:{id}")
}

pub(crate) async fn clear_email_rate_limits(
    tx: &mut Transaction<'_, Postgres>,
    email: &str,
) -> Result<(), sqlx::Error> {
    let keys = [
        "register",
        "forgot",
        "login",
        "delete_account",
        "google_link",
    ]
    .map(|scope| email_bucket(scope, email));
    sqlx::query!(
        "DELETE FROM auth_rate_limits WHERE bucket = ANY($1)",
        &keys[..]
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn hit_bucket(pool: &PgPool, key: &str, limit: i32) -> Result<(), AuthError> {
    let count = sqlx::query_scalar!(
        "INSERT INTO auth_rate_limits (bucket, hits, reset_at) VALUES ($1, 1, now() + interval '15 minutes')
         ON CONFLICT (bucket) DO UPDATE SET
            hits = CASE WHEN auth_rate_limits.reset_at <= now() THEN 1 ELSE auth_rate_limits.hits + 1 END,
            reset_at = CASE WHEN auth_rate_limits.reset_at <= now() THEN EXCLUDED.reset_at ELSE auth_rate_limits.reset_at END
         WHERE auth_rate_limits.reset_at <= now() OR auth_rate_limits.hits < $2 RETURNING hits",
        key, limit,
    ).fetch_optional(pool).await?;
    count.map(|_| ()).ok_or(AuthError::RateLimited)
}
