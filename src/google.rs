use std::net::IpAddr;

use serde::Deserialize;
use sqlx::{PgPool, Postgres, Transaction};
use url::Url;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    AccessKeys,
    accounts::{Accounts, AuthError, check_rate, new_token, token_hash},
    profile,
    sessions::{Identity, TokenPair, active_installation, create_session},
};

mod provider;
pub(crate) mod routes;
#[cfg(test)]
mod tests;

pub use provider::Google;
use provider::{pkce_challenge, valid_challenge, valid_verifier};

#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
struct Start {
    desktop_port: u16,
    desktop_state: String,
    code_challenge: String,
    terms_version: String,
    privacy_policy_version: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proof {
    #[schema(min_length = 64, max_length = 64)]
    code: String,
    #[schema(min_length = 43, max_length = 128)]
    code_verifier: String,
}

struct Handoff {
    code_hash: Vec<u8>,
    subject: String,
    email: String,
    terms_version: String,
    privacy_policy_version: String,
}

async fn start(
    pool: &PgPool,
    accounts: &Accounts,
    google: &Google,
    ip: IpAddr,
    request: Start,
) -> Result<Url, AuthError> {
    if request.desktop_port < 1024
        || !valid_challenge(&request.desktop_state)
        || !valid_challenge(&request.code_challenge)
    {
        return Err(AuthError::InvalidOAuth);
    }
    if request.terms_version != accounts.config().terms_version
        || request.privacy_policy_version != accounts.config().privacy_policy_version
    {
        return Err(AuthError::InvalidConsent);
    }
    check_rate(pool, "google_start", ip, None).await?;
    let (state, hash) = new_token()?;
    sqlx::query!(
        "INSERT INTO google_attempts (state_hash, desktop_port, desktop_state, desktop_challenge,
            terms_version, privacy_policy_version, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '10 minutes')",
        &hash[..],
        i32::from(request.desktop_port),
        request.desktop_state,
        request.code_challenge,
        request.terms_version,
        request.privacy_policy_version,
    )
    .execute(pool)
    .await?;
    Ok(google.authorization_url(&state))
}

async fn callback(
    pool: &PgPool,
    google: &Google,
    state: &str,
    code: Option<&str>,
) -> Result<Url, AuthError> {
    let hash = token_hash(state).map_err(|_| AuthError::InvalidOAuth)?;
    let attempt = sqlx::query!(
        "DELETE FROM google_attempts WHERE state_hash = $1 AND expires_at > now()
         RETURNING desktop_port, desktop_state, desktop_challenge, terms_version, privacy_policy_version",
        &hash[..],
    ).fetch_optional(pool).await?.ok_or(AuthError::InvalidOAuth)?;

    let port = u16::try_from(attempt.desktop_port).map_err(|_| AuthError::Unavailable)?;
    let mut desktop = Url::parse(&format!("http://127.0.0.1:{port}/oauth/moby"))
        .map_err(|_| AuthError::Unavailable)?;
    desktop
        .query_pairs_mut()
        .append_pair("state", &attempt.desktop_state);

    let Some(code) = code.filter(|code| {
        (1..=2048).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_graphic())
    }) else {
        desktop
            .query_pairs_mut()
            .append_pair("error", "authorization_failed");
        return Ok(desktop);
    };
    let identity = match google.exchange(state, code).await {
        Ok(identity) => identity,
        Err(error) => {
            let message = if matches!(error, AuthError::ProviderUnavailable) {
                "provider_unavailable"
            } else {
                "authorization_failed"
            };
            desktop.query_pairs_mut().append_pair("error", message);
            return Ok(desktop);
        }
    };

    let (handoff, hash) = new_token()?;
    sqlx::query!(
        "INSERT INTO google_handoffs (code_hash, desktop_challenge, subject, email,
            terms_version, privacy_policy_version, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '90 seconds')",
        &hash[..],
        attempt.desktop_challenge,
        identity.subject,
        identity.email,
        attempt.terms_version,
        attempt.privacy_policy_version,
    )
    .execute(pool)
    .await?;
    desktop.query_pairs_mut().append_pair("code", &handoff);
    Ok(desktop)
}

async fn exchange(
    pool: &PgPool,
    accounts: &Accounts,
    keys: &AccessKeys,
    ip: IpAddr,
    proof: Proof,
    installation: Option<Uuid>,
) -> Result<TokenPair, AuthError> {
    check_rate(pool, "google_exchange", ip, None).await?;
    let mut tx = pool.begin().await?;
    let handoff = load_handoff(&mut tx, &proof).await?;
    let user_id = match identity_owner(&mut tx, &handoff.subject).await? {
        Some(user_id) => user_id,
        None => create_google_account(&mut tx, accounts, &handoff).await?,
    };
    lock_user(&mut tx, user_id).await?;
    consume(&mut tx, &handoff).await?;
    let pair = create_session(&mut tx, keys, user_id, installation).await?;
    tx.commit().await?;
    Ok(pair)
}

async fn create_google_account(
    tx: &mut Transaction<'_, Postgres>,
    accounts: &Accounts,
    handoff: &Handoff,
) -> Result<Uuid, AuthError> {
    if handoff.terms_version != accounts.config().terms_version
        || handoff.privacy_policy_version != accounts.config().privacy_policy_version
    {
        return Err(AuthError::InvalidConsent);
    }
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (id, email, email_verified_at) VALUES ($1, $2, now())
         ON CONFLICT (email) DO NOTHING RETURNING id",
        Uuid::new_v4(),
        handoff.email,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some(user_id) = user_id else {
        return identity_owner(tx, &handoff.subject)
            .await?
            .ok_or(AuthError::AccountLinkRequired);
    };
    insert_identity(tx, &handoff.subject, user_id).await?;
    sqlx::query!(
        "INSERT INTO consents (user_id, type, version)
            VALUES ($1, 'terms', $2), ($1, 'privacy_policy', $3)",
        user_id,
        handoff.terms_version,
        handoff.privacy_policy_version,
    )
    .execute(&mut **tx)
    .await?;
    Ok(user_id)
}

async fn link(
    pool: &PgPool,
    accounts: &Accounts,
    identity: Identity,
    ip: IpAddr,
    proof: Proof,
    password: String,
) -> Result<(), AuthError> {
    let current =
        profile::confirm_password(pool, accounts, identity, ip, "google_link", password).await?;
    let mut tx = pool.begin().await?;
    let handoff = load_handoff(&mut tx, &proof).await?;
    profile::lock_password_user(&mut tx, identity, &current.hash).await?;
    if current.email != handoff.email {
        return Err(AuthError::IdentityConflict);
    }
    consume(&mut tx, &handoff).await?;
    insert_identity(&mut tx, &handoff.subject, identity.user_id).await?;
    tx.commit().await?;
    tracing::info!("Google identity linked");
    Ok(())
}

pub(crate) async fn delete_account(
    pool: &PgPool,
    identity: Identity,
    ip: IpAddr,
    proof: Proof,
) -> Result<(), AuthError> {
    check_rate(pool, "google_delete", ip, None).await?;
    let mut tx = pool.begin().await?;
    let handoff = load_handoff(&mut tx, &proof).await?;
    let email = lock_user(&mut tx, identity.user_id).await?;
    if identity_owner(&mut tx, &handoff.subject).await? != Some(identity.user_id)
        || active_installation(&mut *tx, identity.session_id, identity.user_id)
            .await?
            .is_none()
    {
        return Err(AuthError::Unauthorized);
    }
    consume(&mut tx, &handoff).await?;
    profile::delete_locked_account(&mut tx, identity.user_id, &email).await?;
    tx.commit().await?;
    tracing::info!("account deleted");
    Ok(())
}

async fn load_handoff(
    tx: &mut Transaction<'_, Postgres>,
    proof: &Proof,
) -> Result<Handoff, AuthError> {
    if !valid_verifier(&proof.code_verifier) {
        return Err(AuthError::InvalidOAuth);
    }
    let hash = token_hash(&proof.code).map_err(|_| AuthError::InvalidOAuth)?;
    let challenge = pkce_challenge(&proof.code_verifier);
    sqlx::query_as!(
        Handoff,
        "SELECT code_hash, subject, email, terms_version, privacy_policy_version
         FROM google_handoffs WHERE code_hash = $1 AND desktop_challenge = $2 AND expires_at > now()",
        &hash[..], challenge,
    ).fetch_optional(&mut **tx).await?.ok_or(AuthError::InvalidOAuth)
}

async fn consume(tx: &mut Transaction<'_, Postgres>, handoff: &Handoff) -> Result<(), AuthError> {
    let removed = sqlx::query!(
        "DELETE FROM google_handoffs WHERE code_hash = $1 AND expires_at > clock_timestamp()",
        &handoff.code_hash[..],
    )
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if removed != 1 {
        return Err(AuthError::InvalidOAuth);
    }
    Ok(())
}

async fn lock_user(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> Result<String, AuthError> {
    sqlx::query_scalar!(
        "SELECT email FROM users WHERE id = $1 AND status = 'active'
         AND email_verified_at IS NOT NULL FOR UPDATE",
        user_id,
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AuthError::Unauthorized)
}

async fn identity_owner(
    tx: &mut Transaction<'_, Postgres>,
    subject: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT user_id FROM oauth_identities WHERE provider = 'google' AND subject = $1",
        subject,
    )
    .fetch_optional(&mut **tx)
    .await
}

async fn insert_identity(
    tx: &mut Transaction<'_, Postgres>,
    subject: &str,
    user_id: Uuid,
) -> Result<(), AuthError> {
    let inserted = sqlx::query!(
        "INSERT INTO oauth_identities (provider, subject, user_id) VALUES ('google', $1, $2)
         ON CONFLICT DO NOTHING",
        subject,
        user_id,
    )
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if inserted == 0 && identity_owner(tx, subject).await? != Some(user_id) {
        return Err(AuthError::IdentityConflict);
    }
    Ok(())
}

pub(crate) async fn purge_user_handoffs(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    email: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM google_handoffs WHERE email = $2 OR subject IN
         (SELECT subject FROM oauth_identities WHERE user_id = $1 AND provider = 'google')",
        user_id,
        email,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn prune(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM google_attempts WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    sqlx::query!("DELETE FROM google_handoffs WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(())
}
