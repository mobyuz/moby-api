CREATE TABLE agent_usage_reservations (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    utc_day DATE NOT NULL,
    reserved_tokens BIGINT NOT NULL CHECK (reserved_tokens > 0),
    reserved_cost_microusd BIGINT NOT NULL CHECK (reserved_cost_microusd >= 0),
    actual_tokens BIGINT CHECK (actual_tokens BETWEEN 0 AND reserved_tokens),
    actual_cost_microusd BIGINT CHECK (actual_cost_microusd BETWEEN 0 AND reserved_cost_microusd),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    settled_at TIMESTAMPTZ,
    CHECK (
        (actual_tokens IS NULL AND actual_cost_microusd IS NULL AND settled_at IS NULL)
        OR (actual_tokens IS NOT NULL AND actual_cost_microusd IS NOT NULL AND settled_at IS NOT NULL)
    )
);

CREATE INDEX agent_usage_reservations_user_day ON agent_usage_reservations (user_id, utc_day)
    WHERE user_id IS NOT NULL;

CREATE TABLE agent_budget_periods (
    period_start DATE NOT NULL,
    monthly BOOLEAN NOT NULL,
    cost_microusd BIGINT NOT NULL CHECK (cost_microusd >= 0),
    PRIMARY KEY (period_start, monthly)
);
