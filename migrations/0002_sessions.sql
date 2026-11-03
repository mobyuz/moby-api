CREATE TABLE installations (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    os TEXT NOT NULL DEFAULT 'windows' CHECK (os = 'windows'),
    app_version TEXT CHECK (length(app_version) BETWEEN 1 AND 64),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    UNIQUE (id, user_id)
);

CREATE INDEX installations_user ON installations (user_id);

CREATE TABLE sessions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    installation_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    FOREIGN KEY (installation_id, user_id) REFERENCES installations (id, user_id) ON DELETE CASCADE
);

CREATE INDEX sessions_user_active ON sessions (user_id) WHERE revoked_at IS NULL;
CREATE INDEX sessions_installation ON sessions (installation_id, user_id);
CREATE INDEX sessions_expiry ON sessions (expires_at);

CREATE TABLE refresh_tokens (
    token_hash BYTEA PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    session_id UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    rotated_at TIMESTAMPTZ
);

CREATE INDEX refresh_tokens_session ON refresh_tokens (session_id);
