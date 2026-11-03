use moby_protocol::{UsageInfo, ZonedDateTime};
use sqlx::{Executor, FromRow, PgPool, Postgres, Transaction};
use time::{Date, format_description::well_known::Rfc3339};
use uuid::Uuid;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
pub struct UsageConfig {
    pub agent_enabled: bool,
    pub voice_enabled: bool,
    pub daily_tokens: u32,
    pub daily_stt_seconds: u32,
    pub daily_tts_chars: u32,
    pub daily_budget_microusd: i64,
    pub monthly_budget_microusd: i64,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            agent_enabled: true,
            voice_enabled: false,
            daily_tokens: 250_000,
            daily_stt_seconds: 300,
            daily_tts_chars: 5_000,
            daily_budget_microusd: 0,
            monthly_budget_microusd: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Charge {
    pub(crate) units: u32,
    pub(crate) cost_microusd: i64,
}

#[derive(Clone, Copy)]
pub(crate) enum Work<'a> {
    Agent,
    Stt(&'a str),
    Tts(&'a str),
}

impl Work<'_> {
    fn meter(self) -> &'static str {
        match self {
            Self::Agent => "llm",
            Self::Stt(_) => "stt",
            Self::Tts(_) => "tts",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UsageError {
    Unauthorized,
    VoiceConsentRequired,
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
    let used = current_day_usage(pool, user).await?;
    let midnight = used
        .day
        .next_day()
        .ok_or(UsageError::Unavailable)?
        .midnight()
        .assume_utc()
        .format(&Rfc3339)
        .map_err(|_| UsageError::Unavailable)?;
    Ok(UsageInfo {
        llm_tokens_remaining: remaining(limits.daily_tokens, used.llm)?,
        stt_seconds_remaining: remaining(
            if limits.voice_enabled {
                limits.daily_stt_seconds
            } else {
                0
            },
            used.stt,
        )?,
        tts_chars_remaining: remaining(
            if limits.voice_enabled {
                limits.daily_tts_chars
            } else {
                0
            },
            used.tts,
        )?,
        reset_at: ZonedDateTime::try_from(midnight).map_err(|_| UsageError::Unavailable)?,
    })
}

fn remaining(cap: u32, used: i64) -> Result<u32, UsageError> {
    let used = u32::try_from(used).map_err(|_| UsageError::Unavailable)?;
    Ok(cap.saturating_sub(used))
}

pub(crate) async fn reserve(
    pool: &PgPool,
    limits: UsageConfig,
    user: Uuid,
    id: Uuid,
    work: Work<'_>,
    charge: Charge,
) -> Result<(), UsageError> {
    if charge.units == 0 || charge.cost_microusd < 0 {
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
    if let Work::Stt(version) | Work::Tts(version) = work {
        // Serialize the dispatch decision with a concurrent withdrawal.
        let consent = sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM consents WHERE user_id = $1 AND type = 'cloud_voice'
             AND version = $2 AND revoked_at IS NULL FOR SHARE",
        )
        .bind(user)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        if consent.is_none() {
            return Err(UsageError::VoiceConsentRequired);
        }
    }
    let usage = current_day_usage(&mut *transaction, user).await?;
    let (cap, used) = match work {
        Work::Agent => (limits.daily_tokens, usage.llm),
        Work::Stt(_) => (limits.daily_stt_seconds, usage.stt),
        Work::Tts(_) => (limits.daily_tts_chars, usage.tts),
    };
    let units = i64::from(charge.units);
    if units > i64::from(cap) - used {
        return Err(UsageError::QuotaExceeded);
    }

    // Lock daily and monthly budget rows in the same order in reserve and settle.
    if charge.cost_microusd > 0 {
        for (period, monthly) in periods(usage.day) {
            let cap = if monthly {
                limits.monthly_budget_microusd
            } else {
                limits.daily_budget_microusd
            };
            if charge.cost_microusd > cap {
                return Err(UsageError::BudgetExhausted);
            }
            let updated = sqlx::query(
                "INSERT INTO usage_budget_periods (period_start, monthly, cost_microusd)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (period_start, monthly) DO UPDATE
                    SET cost_microusd = usage_budget_periods.cost_microusd + EXCLUDED.cost_microusd
                    WHERE usage_budget_periods.cost_microusd <= $4 - EXCLUDED.cost_microusd",
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
        "INSERT INTO usage_reservations (id, user_id, utc_day, meter, reserved_units, reserved_cost_microusd)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(user)
    .bind(usage.day)
    .bind(work.meter())
    .bind(units)
    .bind(charge.cost_microusd)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

#[derive(FromRow)]
struct Reservation {
    utc_day: Date,
    reserved_units: i64,
    reserved_cost_microusd: i64,
    actual_units: Option<i64>,
    actual_cost_microusd: Option<i64>,
}

pub(crate) async fn settle(pool: &PgPool, id: Uuid, charge: Charge) -> Result<(), UsageError> {
    let mut transaction = accounting_transaction(pool).await?;
    let reservation = sqlx::query_as::<_, Reservation>(
        "SELECT utc_day, reserved_units, reserved_cost_microusd, actual_units, actual_cost_microusd
         FROM usage_reservations WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    let units = i64::from(charge.units);
    if let Some(actual) = reservation.actual_units {
        if actual != units || reservation.actual_cost_microusd != Some(charge.cost_microusd) {
            return Err(UsageError::Unavailable);
        }
        transaction.commit().await?;
        return Ok(());
    }
    if units > reservation.reserved_units
        || !(0..=reservation.reserved_cost_microusd).contains(&charge.cost_microusd)
    {
        return Err(UsageError::Unavailable);
    }

    let refund = reservation.reserved_cost_microusd - charge.cost_microusd;
    if refund > 0 {
        for (period, monthly) in periods(reservation.utc_day) {
            let updated = sqlx::query(
                "UPDATE usage_budget_periods SET cost_microusd = cost_microusd - $3
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
        "UPDATE usage_reservations
         SET actual_units = $2, actual_cost_microusd = $3, settled_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(units)
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

#[derive(FromRow)]
struct DayUsage {
    day: Date,
    llm: i64,
    stt: i64,
    tts: i64,
}

async fn current_day_usage<'a>(
    executor: impl Executor<'a, Database = Postgres>,
    user: Uuid,
) -> Result<DayUsage, sqlx::Error> {
    sqlx::query_as(
        "SELECT current_period.day,
            COALESCE(SUM(COALESCE(actual_units, reserved_units)) FILTER (WHERE meter = 'llm'), 0)::bigint AS llm,
            COALESCE(SUM(COALESCE(actual_units, reserved_units)) FILTER (WHERE meter = 'stt'), 0)::bigint AS stt,
            COALESCE(SUM(COALESCE(actual_units, reserved_units)) FILTER (WHERE meter = 'tts'), 0)::bigint AS tts
         FROM (SELECT (statement_timestamp() AT TIME ZONE 'UTC')::date AS day) current_period
         LEFT JOIN usage_reservations ON user_id = $1 AND utc_day = current_period.day
         GROUP BY current_period.day",
    )
    .bind(user)
    .fetch_one(executor)
    .await
}

fn periods(day: Date) -> [(Date, bool); 2] {
    let month = Date::from_calendar_date(day.year(), day.month(), 1).expect("first day of month");
    [(day, false), (month, true)]
}
