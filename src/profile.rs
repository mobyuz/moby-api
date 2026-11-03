use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use sqlx::{Executor, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    accounts::{Accounts, AuthError, check_rate, clear_email_rate_limits, validate_password},
    config::{AccountConfig, valid_policy_version},
    sessions::{Identity, active_installation},
};

pub(crate) mod routes;

#[derive(Serialize, ToSchema)]
struct Profile {
    id: Uuid,
    email: String,
    display_name: Option<String>,
    installation_id: Uuid,
}

#[derive(Serialize, ToSchema)]
struct ExportAccount {
    id: Uuid,
    email: String,
    display_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    email_verified_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
}

#[derive(Serialize, ToSchema)]
struct ExportInstallation {
    id: Uuid,
    name: String,
    os: String,
    app_version: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    last_seen_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    revoked_at: Option<OffsetDateTime>,
}

#[derive(Serialize, ToSchema)]
struct ExportSession {
    installation_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    revoked_at: Option<OffsetDateTime>,
}

#[derive(Serialize, ToSchema)]
struct ExportIdentity {
    provider: String,
    subject: String,
}

#[derive(Serialize, ToSchema)]
struct AccountExport {
    account: ExportAccount,
    installations: Vec<ExportInstallation>,
    sessions: Vec<ExportSession>,
    consents: Vec<ConsentRecord>,
    identities: Vec<ExportIdentity>,
}

#[derive(Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ConsentKind {
    Terms,
    PrivacyPolicy,
    CloudVoice,
}

impl ConsentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Terms => "terms",
            Self::PrivacyPolicy => "privacy_policy",
            Self::CloudVoice => "cloud_voice",
        }
    }

    fn current_version(self, config: &AccountConfig) -> &str {
        match self {
            Self::Terms => &config.terms_version,
            Self::PrivacyPolicy => &config.privacy_policy_version,
            Self::CloudVoice => &config.cloud_voice_version,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct ConsentRecord {
    #[serde(rename = "type")]
    #[schema(value_type = ConsentKind)]
    kind: String,
    version: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    accepted_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    revoked_at: Option<OffsetDateTime>,
}

#[derive(Serialize, ToSchema)]
struct ConsentVersions {
    terms: String,
    privacy_policy: String,
    cloud_voice: String,
}

#[derive(Serialize, ToSchema)]
struct Consents {
    current_versions: ConsentVersions,
    records: Vec<ConsentRecord>,
}

async fn get(pool: &PgPool, identity: Identity) -> Result<Profile, AuthError> {
    let row = sqlx::query!(
        "SELECT email, display_name FROM users WHERE id = $1",
        identity.user_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AuthError::Unauthorized)?;
    Ok(Profile {
        id: identity.user_id,
        email: row.email,
        display_name: row.display_name,
        installation_id: identity.installation_id,
    })
}

async fn update(
    pool: &PgPool,
    identity: Identity,
    name: Option<&str>,
) -> Result<Profile, AuthError> {
    let name = name.map(str::trim);
    if name.is_some_and(|name| {
        !(1..=100).contains(&name.chars().count()) || name.chars().any(char::is_control)
    }) {
        return Err(AuthError::InvalidProfileName);
    }
    let row = sqlx::query!(
        "UPDATE users SET display_name = $2 WHERE id = $1 AND status = 'active'
         RETURNING email, display_name",
        identity.user_id,
        name,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AuthError::Unauthorized)?;
    Ok(Profile {
        id: identity.user_id,
        email: row.email,
        display_name: row.display_name,
        installation_id: identity.installation_id,
    })
}

async fn export(pool: &PgPool, user_id: Uuid) -> Result<AccountExport, AuthError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let account = sqlx::query_as!(
        ExportAccount,
        r#"SELECT id, email, display_name, email_verified_at AS "email_verified_at!", created_at
           FROM users WHERE id = $1 AND email_verified_at IS NOT NULL AND status = 'active'"#,
        user_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AuthError::Unauthorized)?;
    let installations = sqlx::query_as!(
        ExportInstallation,
        "SELECT id, name, os, app_version, created_at, last_seen_at, revoked_at
         FROM installations WHERE user_id = $1 ORDER BY created_at, id",
        user_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let sessions = sqlx::query_as!(
        ExportSession,
        "SELECT installation_id, created_at, expires_at, revoked_at
         FROM sessions WHERE user_id = $1 ORDER BY created_at, id",
        user_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let consents = consent_records(&mut *tx, user_id).await?;
    let identities = sqlx::query_as!(
        ExportIdentity,
        "SELECT provider, subject FROM oauth_identities WHERE user_id = $1 ORDER BY provider",
        user_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(AccountExport {
        account,
        installations,
        sessions,
        consents,
        identities,
    })
}

async fn delete(
    pool: &PgPool,
    accounts: &Accounts,
    identity: Identity,
    ip: IpAddr,
    password: String,
) -> Result<(), AuthError> {
    let current =
        confirm_password(pool, accounts, identity, ip, "delete_account", password).await?;
    let mut tx = pool.begin().await?;
    lock_password_user(&mut tx, identity, &current.hash).await?;
    delete_locked_account(&mut tx, identity.user_id, &current.email).await?;
    tx.commit().await?;
    tracing::info!("account deleted");
    Ok(())
}

pub(crate) struct CurrentPassword {
    pub(crate) email: String,
    pub(crate) hash: String,
}

pub(crate) async fn confirm_password(
    pool: &PgPool,
    accounts: &Accounts,
    identity: Identity,
    ip: IpAddr,
    scope: &str,
    password: String,
) -> Result<CurrentPassword, AuthError> {
    validate_password(&password).map_err(|_| AuthError::Unauthorized)?;
    let user = sqlx::query!(
        r#"SELECT email, password_hash AS "password_hash!"
           FROM users WHERE id = $1 AND status = 'active'
             AND email_verified_at IS NOT NULL AND password_hash IS NOT NULL"#,
        identity.user_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AuthError::Unauthorized)?;
    check_rate(pool, scope, ip, Some((&user.email, 5))).await?;
    if !accounts
        .password_matches(password, user.password_hash.clone())
        .await?
    {
        return Err(AuthError::Unauthorized);
    }

    Ok(CurrentPassword {
        email: user.email,
        hash: user.password_hash,
    })
}

pub(crate) async fn lock_password_user(
    tx: &mut Transaction<'_, Postgres>,
    identity: Identity,
    hash: &str,
) -> Result<(), AuthError> {
    let locked = sqlx::query_scalar!(
        "SELECT id FROM users WHERE id = $1 AND password_hash = $2
         AND status = 'active' AND email_verified_at IS NOT NULL FOR UPDATE",
        identity.user_id,
        hash,
    )
    .fetch_optional(&mut **tx)
    .await?;
    if locked.is_none()
        || active_installation(&mut **tx, identity.session_id, identity.user_id)
            .await?
            .is_none()
    {
        return Err(AuthError::Unauthorized);
    }
    Ok(())
}

pub(crate) async fn delete_locked_account(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    email: &str,
) -> Result<(), sqlx::Error> {
    crate::google::purge_user_handoffs(tx, user_id, email).await?;
    sqlx::query!("DELETE FROM users WHERE id = $1", user_id)
        .execute(&mut **tx)
        .await?;
    clear_email_rate_limits(tx, email).await?;
    Ok(())
}

async fn consent_records<'e>(
    executor: impl Executor<'e, Database = Postgres>,
    user_id: Uuid,
) -> Result<Vec<ConsentRecord>, sqlx::Error> {
    sqlx::query_as!(
        ConsentRecord,
        "SELECT type AS kind, version, accepted_at, revoked_at
         FROM consents WHERE user_id = $1 ORDER BY type, accepted_at, version",
        user_id,
    )
    .fetch_all(executor)
    .await
}

async fn list_consents(
    pool: &PgPool,
    config: &AccountConfig,
    user_id: Uuid,
) -> Result<Consents, AuthError> {
    let records = consent_records(pool, user_id).await?;
    Ok(Consents {
        current_versions: ConsentVersions {
            terms: config.terms_version.clone(),
            privacy_policy: config.privacy_policy_version.clone(),
            cloud_voice: config.cloud_voice_version.clone(),
        },
        records,
    })
}

async fn set_consent(
    pool: &PgPool,
    config: &AccountConfig,
    user_id: Uuid,
    kind: ConsentKind,
    version: &str,
    accepted: bool,
) -> Result<(), AuthError> {
    if !valid_policy_version(version) || (accepted && version != kind.current_version(config)) {
        return Err(AuthError::InvalidConsentVersion);
    }
    if accepted {
        sqlx::query!(
            "INSERT INTO consents (user_id, type, version) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, type, version) DO UPDATE SET accepted_at = now(), revoked_at = NULL
             WHERE consents.revoked_at IS NOT NULL",
            user_id, kind.as_str(), version,
        ).execute(pool).await?;
    } else {
        sqlx::query!(
            "UPDATE consents SET revoked_at = now()
             WHERE user_id = $1 AND type = $2 AND version = $3 AND revoked_at IS NULL",
            user_id,
            kind.as_str(),
            version,
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}
