ALTER TABLE usage_reservations
    ADD COLUMN provider TEXT CHECK (provider IS NULL OR length(provider) BETWEEN 1 AND 32),
    ADD COLUMN model TEXT CHECK (model IS NULL OR length(model) BETWEEN 1 AND 128);
