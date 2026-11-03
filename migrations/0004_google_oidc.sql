CREATE TABLE oauth_identities (
    provider TEXT NOT NULL CHECK (provider = 'google'),
    subject TEXT NOT NULL CHECK (length(subject) BETWEEN 1 AND 255),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (provider, subject),
    UNIQUE (user_id, provider)
);

CREATE TABLE google_attempts (
    state_hash BYTEA PRIMARY KEY CHECK (octet_length(state_hash) = 32),
    desktop_port INTEGER NOT NULL CHECK (desktop_port BETWEEN 1024 AND 65535),
    desktop_state TEXT NOT NULL CHECK (length(desktop_state) = 43),
    desktop_challenge TEXT NOT NULL CHECK (length(desktop_challenge) = 43),
    terms_version TEXT NOT NULL,
    privacy_policy_version TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX google_attempts_expiry ON google_attempts (expires_at);

CREATE TABLE google_handoffs (
    code_hash BYTEA PRIMARY KEY CHECK (octet_length(code_hash) = 32),
    desktop_challenge TEXT NOT NULL CHECK (length(desktop_challenge) = 43),
    subject TEXT NOT NULL CHECK (length(subject) BETWEEN 1 AND 255),
    email TEXT NOT NULL CHECK (length(email) BETWEEN 3 AND 254),
    terms_version TEXT NOT NULL,
    privacy_policy_version TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX google_handoffs_expiry ON google_handoffs (expires_at);
CREATE INDEX google_handoffs_email ON google_handoffs (email);
CREATE INDEX google_handoffs_subject ON google_handoffs (subject);
