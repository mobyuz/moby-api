use std::time::Duration;

use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor, message::Mailbox};
use sqlx::PgPool;
use tokio::{sync::mpsc, time};

use super::{AuthError, RESET_PATH, VERIFY_PATH, new_token};
use crate::config::AccountConfig;

pub(super) enum Job {
    Verify { email: String, token: String },
    Forgot { email: String },
}

pub(super) async fn run(pool: PgPool, config: AccountConfig, mut jobs: mpsc::Receiver<Job>) {
    let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
        .port(config.smtp_port)
        .timeout(Some(Duration::from_secs(5)))
        .build();
    let cleanup_period = Duration::from_secs(15 * 60);
    let mut cleanup = time::interval_at(time::Instant::now() + cleanup_period, cleanup_period);
    cleanup.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            job = jobs.recv() => {
                match job {
                    Some(Job::Verify { email, token }) => {
                        send(&transport, &config, email, &token, VERIFY_PATH).await;
                    }
                    Some(Job::Forgot { email }) => match reset_token(&pool, &email).await {
                        Ok(Some(token)) => send(&transport, &config, email, &token, RESET_PATH).await,
                        Ok(None) => {},
                        Err(_) => tracing::warn!("could not prepare password reset"),
                    },
                    None => break,
                }
            }
            _ = cleanup.tick() => {
                if prune(&pool).await.is_err() {
                    tracing::warn!("could not prune expired authentication records");
                }
            }
        }
    }
}

async fn reset_token(pool: &PgPool, email: &str) -> Result<Option<String>, AuthError> {
    let mut tx = pool.begin().await?;
    let user_id = sqlx::query_scalar!(
        "SELECT id FROM users WHERE email = $1 AND status = 'active'
            AND email_verified_at IS NOT NULL AND password_hash IS NOT NULL FOR UPDATE",
        email,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(user_id) = user_id else {
        return Ok(None);
    };

    let (token, hash) = new_token()?;
    sqlx::query!(
        "INSERT INTO email_tokens (token_hash, user_id, kind, expires_at)
         VALUES ($1, $2, 'password_reset', now() + interval '30 minutes')",
        &hash[..],
        user_id,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(token))
}

async fn prune(pool: &PgPool) -> Result<(), sqlx::Error> {
    crate::sessions::prune(pool).await?;
    crate::google::prune(pool).await?;
    sqlx::query!("DELETE FROM auth_rate_limits WHERE reset_at <= now()")
        .execute(pool)
        .await?;
    sqlx::query!("DELETE FROM email_tokens WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(())
}

async fn send(
    transport: &AsyncSmtpTransport<Tokio1Executor>,
    config: &AccountConfig,
    email: String,
    token: &str,
    path: &str,
) {
    let Ok(recipient) = email.parse() else {
        return;
    };
    let mut link = config.base_url.clone();
    link.set_path(path);
    link.set_fragment(Some(&format!("token={token}")));
    let subject = if path == VERIFY_PATH {
        "Verify your Moby email"
    } else {
        "Reset your Moby password"
    };
    let text =
        format!("{subject}\n\n{link}\n\nIf you did not request this, you can ignore this email.");
    let message = Message::builder()
        .from(Mailbox::new(None, config.mail_from.clone()))
        .to(Mailbox::new(None, recipient))
        .subject(subject)
        .body(text);
    let Ok(message) = message else {
        tracing::warn!("could not build account email");
        return;
    };
    if transport.send(message).await.is_err() {
        tracing::warn!("account email delivery failed");
    }
}
