-- paur auth: server-side sessions for the admin UI.
--
-- The session token is a 256-bit random value sent to the browser as a
-- cookie. We store the SHA-256 hash of the token in `token_hash` so a
-- leaked DB does not let the attacker impersonate live sessions.
-- Plaintext tokens are never persisted.

CREATE TABLE IF NOT EXISTS sessions (
    token_hash  TEXT PRIMARY KEY,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    user        TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);
