ALTER TABLE agent_usage_reservations RENAME TO usage_reservations;
ALTER TABLE usage_reservations RENAME COLUMN reserved_tokens TO reserved_units;
ALTER TABLE usage_reservations RENAME COLUMN actual_tokens TO actual_units;
ALTER TABLE usage_reservations
    ADD COLUMN meter TEXT NOT NULL DEFAULT 'llm' CHECK (meter IN ('llm', 'stt', 'tts'));
ALTER TABLE usage_reservations ALTER COLUMN meter DROP DEFAULT;
ALTER INDEX agent_usage_reservations_user_day RENAME TO usage_reservations_user_day;

ALTER TABLE agent_budget_periods RENAME TO usage_budget_periods;
