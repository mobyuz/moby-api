use std::net::IpAddr;

use serde::Serialize;
use sqlx::{Executor, PgPool, Postgres, Transaction};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::accounts::{
    Accounts, AuthError, check_rate, new_token, normalize_email, token_hash, validate_password,
};

pub(crate) mod routes;
pub(crate) mod token;

use token::{ACCESS_SECONDS, AccessKeys};

// Same Argon2id parameters as account passwords. No user owns this synthetic hash.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$bW9ieS1kdW1teS1zYWx0IQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[derive(Serialize, ToSchema)]
pub(crate) struct TokenPair {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    expires_in: u64,
    installation_id: Uuid,
}

#[derive(Serialize, ToSchema)]
struct Installation {
    id: Uuid,
    name: String,
    os: String,
    app_version: Option<String>,
    revoked: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Identity {
    pub(crate) user_id: Uuid,
    pub(crate) session_id: Uuid,
    pub(crate) installation_id: Uuid,
}

async fn login(
    pool: &PgPool,
    accounts: &Accounts,
    keys: &AccessKeys,
    ip: IpAddr,
    email: String,
    password: String,
    installation: Option<Uuid>,
) -> Result<TokenPair, AuthError> {
    let email = normalize_email(&email)?;
    check_rate(pool, "login", ip, Some((&email, 8))).await?;
    validate_password(&password).map_err(|_| AuthError::Unauthorized)?;
    let candidate = sqlx::query!(
        r#"SELECT id, password_hash AS "password_hash!"
           FROM users WHERE email = $1 AND status = 'active'
                AND email_verified_at IS NOT NULL AND password_hash IS NOT NULL"#,
        email,
    )
    .fetch_optional(pool)
    .await?;
    let hash = candidate
        .as_ref()
        .map_or_else(|| DUMMY_HASH.into(), |user| user.password_hash.clone());
    let matched = accounts.password_matches(password, hash).await?;
    let user = candidate
        .filter(|_| matched)
        .ok_or(AuthError::Unauthorized)?;

    let mut tx = pool.begin().await?;
    let user_id = sqlx::query_scalar!(
        "SELECT id FROM users WHERE id = $1 AND password_hash = $2
            AND status = 'active' AND email_verified_at IS NOT NULL FOR UPDATE",
        user.id,
        user.password_hash,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AuthError::Unauthorized)?;

    let pair = create_session(&mut tx, keys, user_id, installation).await?;
    tx.commit().await?;
    Ok(pair)
}

pub(crate) async fn create_session(
    tx: &mut Transaction<'_, Postgres>,
    keys: &AccessKeys,
    user_id: Uuid,
    installation: Option<Uuid>,
) -> Result<TokenPair, AuthError> {
    let installation_id = if let Some(id) = installation {
        sqlx::query_scalar!(
            "UPDATE installations SET last_seen_at = now()
             WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL RETURNING id",
            id,
            user_id,
        )
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AuthError::Unauthorized)?
    } else {
        create_installation(tx, user_id, "Moby Desktop", None)
            .await?
            .id
    };

    let session_id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO sessions (id, user_id, installation_id, expires_at)
            VALUES ($1, $2, $3, now() + interval '30 days')",
        session_id,
        user_id,
        installation_id,
    )
    .execute(&mut **tx)
    .await?;
    issue(tx, keys, user_id, session_id, installation_id).await
}

async fn refresh(
    pool: &PgPool,
    keys: &AccessKeys,
    ip: IpAddr,
    token: &str,
) -> Result<TokenPair, AuthError> {
    check_rate(pool, "refresh", ip, None).await?;
    let hash = token_hash(token).map_err(|_| AuthError::Unauthorized)?;
    let mut tx = pool.begin().await?;
    let family = sqlx::query!(
        "SELECT s.id, s.user_id, s.installation_id FROM refresh_tokens t
         JOIN sessions s ON s.id = t.session_id
         JOIN users u ON u.id = s.user_id
         JOIN installations i ON i.id = s.installation_id
         WHERE t.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > clock_timestamp()
           AND i.revoked_at IS NULL AND u.status = 'active' AND u.email_verified_at IS NOT NULL
         FOR UPDATE OF s",
        &hash[..],
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AuthError::Unauthorized)?;
    let consumed = sqlx::query!(
        "UPDATE refresh_tokens SET rotated_at = now()
            WHERE token_hash = $1 AND rotated_at IS NULL",
        &hash[..],
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if consumed == 0 {
        sqlx::query!(
            "UPDATE sessions SET revoked_at = now() WHERE id = $1",
            family.id
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!("refresh token reuse revoked a session family");
        return Err(AuthError::Unauthorized);
    }
    let pair = issue(
        &mut tx,
        keys,
        family.user_id,
        family.id,
        family.installation_id,
    )
    .await?;
    tx.commit().await?;
    Ok(pair)
}

async fn logout(pool: &PgPool, ip: IpAddr, token: &str) -> Result<(), AuthError> {
    check_rate(pool, "logout", ip, None).await?;
    let hash = token_hash(token).map_err(|_| AuthError::Unauthorized)?;
    sqlx::query!(
        "UPDATE sessions s SET revoked_at = now() FROM refresh_tokens t
         WHERE t.token_hash = $1 AND t.session_id = s.id AND s.revoked_at IS NULL",
        &hash[..],
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn issue(
    tx: &mut Transaction<'_, Postgres>,
    keys: &AccessKeys,
    user_id: Uuid,
    session_id: Uuid,
    installation_id: Uuid,
) -> Result<TokenPair, AuthError> {
    let access_token = keys.sign(user_id, session_id)?;
    let (refresh_token, hash) = new_token()?;
    sqlx::query!(
        "INSERT INTO refresh_tokens (token_hash, session_id) VALUES ($1, $2)",
        &hash[..],
        session_id,
    )
    .execute(&mut **tx)
    .await?;
    Ok(TokenPair {
        access_token,
        refresh_token,
        token_type: "Bearer",
        expires_in: ACCESS_SECONDS,
        installation_id,
    })
}

async fn authorize(
    pool: &PgPool,
    keys: &AccessKeys,
    access_token: &str,
) -> Result<Identity, AuthError> {
    let claims = keys.verify(access_token)?;
    let installation_id = active_installation(pool, claims.sid, claims.sub)
        .await?
        .ok_or(AuthError::Unauthorized)?;
    Ok(Identity {
        user_id: claims.sub,
        session_id: claims.sid,
        installation_id,
    })
}

pub(crate) async fn active_installation<'e>(
    executor: impl Executor<'e, Database = Postgres>,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT s.installation_id FROM sessions s
         JOIN users u ON u.id = s.user_id
         JOIN installations i ON i.id = s.installation_id
         WHERE s.id = $1 AND s.user_id = $2 AND s.revoked_at IS NULL AND s.expires_at > now()
           AND i.revoked_at IS NULL AND u.status = 'active' AND u.email_verified_at IS NOT NULL",
        session_id,
        user_id,
    )
    .fetch_optional(executor)
    .await
}

async fn list_installations(pool: &PgPool, user_id: Uuid) -> Result<Vec<Installation>, AuthError> {
    Ok(sqlx::query_as!(
        Installation,
        r#"SELECT id, name, os, app_version, revoked_at IS NOT NULL AS "revoked!"
           FROM installations WHERE user_id = $1 ORDER BY created_at, id"#,
        user_id,
    )
    .fetch_all(pool)
    .await?)
}

async fn add_installation(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    app_version: Option<&str>,
) -> Result<Installation, AuthError> {
    let name = name.trim();
    if !(1..=100).contains(&name.chars().count())
        || name.chars().any(char::is_control)
        || app_version.is_some_and(|value| {
            value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".+-_".contains(&byte))
        })
    {
        return Err(AuthError::InvalidInstallation);
    }
    let mut tx = pool.begin().await?;
    let installation = create_installation(&mut tx, user_id, name, app_version).await?;
    tx.commit().await?;
    Ok(installation)
}

async fn create_installation(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    name: &str,
    app_version: Option<&str>,
) -> Result<Installation, sqlx::Error> {
    sqlx::query_as!(
        Installation,
        r#"INSERT INTO installations (id, user_id, name, app_version) VALUES ($1, $2, $3, $4)
            RETURNING id, name, os, app_version, false AS "revoked!""#,
        Uuid::new_v4(),
        user_id,
        name,
        app_version,
    )
    .fetch_one(&mut **tx)
    .await
}

async fn revoke_installation(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<(), AuthError> {
    let mut tx = pool.begin().await?;
    let found = sqlx::query!(
        "UPDATE installations SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1 AND user_id = $2",
        id, user_id,
    ).execute(&mut *tx).await?.rows_affected();
    if found == 0 {
        return Err(AuthError::NotFound);
    }
    sqlx::query!(
        "UPDATE sessions SET revoked_at = now()
            WHERE installation_id = $1 AND user_id = $2 AND revoked_at IS NULL",
        id,
        user_id,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn revoke_user_sessions(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
        user_id
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn prune(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM sessions WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(())
}
