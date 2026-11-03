use moby_protocol::{UsageInfo, ZonedDateTime};
use sqlx::{Executor, FromRow, PgPool, Postgres, Transaction};
use time::{Date, format_description::well_known::Rfc3339};
use uuid::Uuid;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
pub struct UsageConfig {
    pub agent_enabled: bool,
    pub daily_tokens: u32,
    pub daily_budget_microusd: i64,
    pub monthly_budget_microusd: i64,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            agent_enabled: true,
            daily_tokens: 250_000,
            daily_budget_microusd: 0,
            monthly_budget_microusd: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Charge {
    pub(crate) tokens: u32,
    pub(crate) cost_microusd: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UsageError {
    Unauthorized,
    QuotaExceeded,
    BudgetExhausted,
    Unavailable,
}

impl From<sqlx::Error> for UsageError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

pub(crate) async fn current(
    pool: &PgPool,
    limits: UsageConfig,
    user: Uuid,
) -> Result<UsageInfo, UsageError> {
    let (day, used) = current_day_usage(pool, user).await?;
    let midnight = day
        .next_day()
        .ok_or(UsageError::Unavailable)?
        .midnight()
        .assume_utc()
        .format(&Rfc3339)
        .map_err(|_| UsageError::Unavailable)?;
    Ok(UsageInfo {
        llm_tokens_remaining: limits
            .daily_tokens
            .saturating_sub(u32::try_from(used).map_err(|_| UsageError::Unavailable)?),
        stt_seconds_remaining: 0,
        tts_chars_remaining: 0,
        reset_at: ZonedDateTime::try_from(midnight).map_err(|_| UsageError::Unavailable)?,
    })
}

pub(crate) async fn reserve(
    pool: &PgPool,
    limits: UsageConfig,
    user: Uuid,
    id: Uuid,
    charge: Charge,
) -> Result<(), UsageError> {
    if charge.tokens == 0 || charge.cost_microusd < 0 {
        return Err(UsageError::Unavailable);
    }
    let mut transaction = accounting_transaction(pool).await?;
    let active = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE id = $1 AND status = 'active'
           AND email_verified_at IS NOT NULL FOR NO KEY UPDATE",
    )
    .bind(user)
    .fetch_optional(&mut *transaction)
    .await?;
    if active.is_none() {
        return Err(UsageError::Unauthorized);
    }
    let (day, used) = current_day_usage(&mut *transaction, user).await?;
    let tokens = i64::from(charge.tokens);
    if tokens > i64::from(limits.daily_tokens) - used {
        return Err(UsageError::QuotaExceeded);
    }

    // Lock daily and monthly budget rows in the same order in reserve and settle.
    if charge.cost_microusd > 0 {
        for (period, monthly) in periods(day) {
            let cap = if monthly {
                limits.monthly_budget_microusd
            } else {
                limits.daily_budget_microusd
            };
            if charge.cost_microusd > cap {
                return Err(UsageError::BudgetExhausted);
            }
            let updated = sqlx::query(
                "INSERT INTO agent_budget_periods (period_start, monthly, cost_microusd)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (period_start, monthly) DO UPDATE
                    SET cost_microusd = agent_budget_periods.cost_microusd + EXCLUDED.cost_microusd
                    WHERE agent_budget_periods.cost_microusd <= $4 - EXCLUDED.cost_microusd",
            )
            .bind(period)
            .bind(monthly)
            .bind(charge.cost_microusd)
            .bind(cap)
            .execute(&mut *transaction)
            .await?;
            if updated.rows_affected() == 0 {
                return Err(UsageError::BudgetExhausted);
            }
        }
    }
    sqlx::query(
        "INSERT INTO agent_usage_reservations (id, user_id, utc_day, reserved_tokens, reserved_cost_microusd)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(user)
    .bind(day)
    .bind(tokens)
    .bind(charge.cost_microusd)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

#[derive(FromRow)]
struct Reservation {
    utc_day: Date,
    reserved_tokens: i64,
    reserved_cost_microusd: i64,
    actual_tokens: Option<i64>,
    actual_cost_microusd: Option<i64>,
}

pub(crate) async fn settle(pool: &PgPool, id: Uuid, charge: Charge) -> Result<(), UsageError> {
    let mut transaction = accounting_transaction(pool).await?;
    let reservation = sqlx::query_as::<_, Reservation>(
        "SELECT utc_day, reserved_tokens, reserved_cost_microusd, actual_tokens, actual_cost_microusd
         FROM agent_usage_reservations WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    let tokens = i64::from(charge.tokens);
    if let Some(actual) = reservation.actual_tokens {
        if actual != tokens || reservation.actual_cost_microusd != Some(charge.cost_microusd) {
            return Err(UsageError::Unavailable);
        }
        transaction.commit().await?;
        return Ok(());
    }
    if tokens > reservation.reserved_tokens
        || !(0..=reservation.reserved_cost_microusd).contains(&charge.cost_microusd)
    {
        return Err(UsageError::Unavailable);
    }

    let refund = reservation.reserved_cost_microusd - charge.cost_microusd;
    if refund > 0 {
        for (period, monthly) in periods(reservation.utc_day) {
            let updated = sqlx::query(
                "UPDATE agent_budget_periods SET cost_microusd = cost_microusd - $3
                 WHERE period_start = $1 AND monthly = $2 AND cost_microusd >= $3",
            )
            .bind(period)
            .bind(monthly)
            .bind(refund)
            .execute(&mut *transaction)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(UsageError::Unavailable);
            }
        }
    }
    sqlx::query(
        "UPDATE agent_usage_reservations
         SET actual_tokens = $2, actual_cost_microusd = $3, settled_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(tokens)
    .bind(charge.cost_microusd)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn accounting_transaction(
    pool: &PgPool,
) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    // The quota lookup after the account lock must see any preceding reservation.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *transaction)
        .await?;
    Ok(transaction)
}

async fn current_day_usage<'a>(
    executor: impl Executor<'a, Database = Postgres>,
    user: Uuid,
) -> Result<(Date, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT utc_day, COALESCE((
            SELECT SUM(COALESCE(actual_tokens, reserved_tokens))
            FROM agent_usage_reservations
            WHERE user_id = $1 AND agent_usage_reservations.utc_day = current_period.utc_day
         ), 0)::bigint
         FROM (SELECT (statement_timestamp() AT TIME ZONE 'UTC')::date AS utc_day) current_period",
    )
    .bind(user)
    .fetch_one(executor)
    .await
}

fn periods(day: Date) -> [(Date, bool); 2] {
    let month = Date::from_calendar_date(day.year(), day.month(), 1).expect("first day of month");
    [(day, false), (month, true)]
}
