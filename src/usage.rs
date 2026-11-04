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

/// Which provider and model handled a reservation. The ledger stores names only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub(crate) provider: &'static str,
    pub(crate) model: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct DemoBudget {
    pub(crate) daily_units: u32,
    pub(crate) daily_cost_microusd: i64,
    pub(crate) monthly_cost_microusd: i64,
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

    reserve_budget(
        &mut transaction,
        usage.day,
        limits.daily_budget_microusd,
        limits.monthly_budget_microusd,
        None,
        charge,
    )
    .await?;
    insert_reservation(
        &mut transaction,
        id,
        Some(user),
        usage.day,
        work.meter(),
        charge,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn reserve_demo(
    pool: &PgPool,
    limits: DemoBudget,
    id: Uuid,
    charge: Charge,
) -> Result<(), UsageError> {
    if charge.units == 0 || charge.cost_microusd < 0 {
        return Err(UsageError::Unavailable);
    }
    let mut transaction = accounting_transaction(pool).await?;
    let day =
        sqlx::query_scalar::<_, Date>("SELECT (statement_timestamp() AT TIME ZONE 'UTC')::date")
            .fetch_one(&mut *transaction)
            .await?;
    reserve_budget(
        &mut transaction,
        day,
        limits.daily_cost_microusd,
        limits.monthly_cost_microusd,
        Some(limits.daily_units),
        charge,
    )
    .await?;
    insert_reservation(&mut transaction, id, None, day, "demo", charge).await?;
    transaction.commit().await?;
    Ok(())
}

/// Records the route on a reservation. Settlement does not depend on it.
pub(crate) async fn label(pool: &PgPool, id: Uuid, route: &Route) -> Result<(), UsageError> {
    sqlx::query("UPDATE usage_reservations SET provider = $2, model = $3 WHERE id = $1")
        .bind(id)
        .bind(route.provider)
        .bind(route.model.as_deref())
        .execute(pool)
        .await?;
    Ok(())
}

async fn insert_reservation(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    user: Option<Uuid>,
    day: Date,
    meter: &str,
    charge: Charge,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_reservations (id, user_id, utc_day, meter, reserved_units, reserved_cost_microusd)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(user)
    .bind(day)
    .bind(meter)
    .bind(i64::from(charge.units))
    .bind(charge.cost_microusd)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn reserve_budget(
    transaction: &mut Transaction<'_, Postgres>,
    day: Date,
    daily_cost: i64,
    monthly_cost: i64,
    guest_daily_units: Option<u32>,
    charge: Charge,
) -> Result<(), UsageError> {
    let guest = guest_daily_units.is_some();
    // Reserve and settle acquire the daily bucket before the monthly bucket.
    for (period, monthly) in periods(day) {
        let (units, unit_cap) = match (monthly, guest_daily_units) {
            (false, Some(cap)) => (i64::from(charge.units), i64::from(cap)),
            _ => (0, 0),
        };
        let cost = charge.cost_microusd;
        if units == 0 && cost == 0 {
            continue;
        }
        let cost_cap = if monthly { monthly_cost } else { daily_cost };
        if units > unit_cap || cost > cost_cap {
            return Err(UsageError::BudgetExhausted);
        }
        let updated = sqlx::query(
            "INSERT INTO usage_budget_periods (period_start, monthly, guest, cost_microusd, units)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (period_start, monthly, guest) DO UPDATE
                SET cost_microusd = usage_budget_periods.cost_microusd + EXCLUDED.cost_microusd,
                    units = usage_budget_periods.units + EXCLUDED.units
                WHERE usage_budget_periods.cost_microusd <= $6 - EXCLUDED.cost_microusd
                  AND usage_budget_periods.units <= $7 - EXCLUDED.units",
        )
        .bind(period)
        .bind(monthly)
        .bind(guest)
        .bind(cost)
        .bind(units)
        .bind(cost_cap)
        .bind(unit_cap)
        .execute(&mut **transaction)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(UsageError::BudgetExhausted);
        }
    }
    Ok(())
}

#[derive(FromRow)]
struct Reservation {
    utc_day: Date,
    reserved_units: i64,
    reserved_cost_microusd: i64,
    actual_units: Option<i64>,
    actual_cost_microusd: Option<i64>,
    guest: bool,
}

pub(crate) async fn settle(pool: &PgPool, id: Uuid, charge: Charge) -> Result<(), UsageError> {
    let mut transaction = accounting_transaction(pool).await?;
    let reservation = sqlx::query_as::<_, Reservation>(
        "SELECT utc_day, reserved_units, reserved_cost_microusd, actual_units, actual_cost_microusd,
                meter = 'demo' AS guest
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

    let cost_refund = reservation.reserved_cost_microusd - charge.cost_microusd;
    for (period, monthly) in periods(reservation.utc_day) {
        let units_refund = if reservation.guest && !monthly {
            reservation.reserved_units - units
        } else {
            0
        };
        if units_refund == 0 && cost_refund == 0 {
            continue;
        }
        let updated = sqlx::query(
            "UPDATE usage_budget_periods SET cost_microusd = cost_microusd - $4, units = units - $5
             WHERE period_start = $1 AND monthly = $2 AND guest = $3
               AND cost_microusd >= $4 AND units >= $5",
        )
        .bind(period)
        .bind(monthly)
        .bind(reservation.guest)
        .bind(cost_refund)
        .bind(units_refund)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(UsageError::Unavailable);
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
