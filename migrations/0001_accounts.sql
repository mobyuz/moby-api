CREATE TABLE users (
    id UUID PRIMARY KEY,
    email TEXT NOT NULL UNIQUE CHECK (email = lower(email) AND length(email) BETWEEN 3 AND 254),
    email_verified_at TIMESTAMPTZ,
    password_hash TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE email_tokens (
    token_hash BYTEA PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('verify_email', 'password_reset')),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    pending_password_hash TEXT,
    terms_version TEXT,
    privacy_policy_version TEXT,
    CHECK (
        (kind = 'verify_email' AND used_at IS NULL AND pending_password_hash IS NOT NULL
            AND terms_version IS NOT NULL AND privacy_policy_version IS NOT NULL)
        OR (pending_password_hash IS NULL AND terms_version IS NULL AND privacy_policy_version IS NULL
            AND (kind = 'password_reset' OR used_at IS NOT NULL))
    )
);

CREATE INDEX email_tokens_active_user_kind ON email_tokens (user_id, kind) WHERE used_at IS NULL;
CREATE INDEX email_tokens_expiry ON email_tokens (expires_at);

CREATE TABLE consents (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    type TEXT NOT NULL CHECK (type IN ('terms', 'privacy_policy', 'cloud_voice')),
    version TEXT NOT NULL CHECK (length(version) BETWEEN 1 AND 64),
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, type, version)
);

CREATE TABLE auth_rate_limits (
    bucket TEXT PRIMARY KEY,
    hits INTEGER NOT NULL CHECK (hits > 0),
    reset_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX auth_rate_limits_expiry ON auth_rate_limits (reset_at);
