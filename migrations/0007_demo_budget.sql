ALTER TABLE usage_reservations
    DROP CONSTRAINT usage_reservations_meter_check,
    ADD CONSTRAINT usage_reservations_meter_check CHECK (meter IN ('llm', 'stt', 'tts', 'demo')),
    ADD CONSTRAINT usage_reservations_demo_has_no_user CHECK (meter <> 'demo' OR user_id IS NULL);

ALTER TABLE usage_budget_periods
    ADD COLUMN guest BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN units BIGINT NOT NULL DEFAULT 0,
    DROP CONSTRAINT agent_budget_periods_pkey,
    ADD PRIMARY KEY (period_start, monthly, guest),
    ADD CONSTRAINT usage_budget_periods_guest_daily_units
        CHECK (units >= 0 AND ((guest AND NOT monthly) OR units = 0));

ALTER TABLE usage_budget_periods ALTER COLUMN guest DROP DEFAULT;
