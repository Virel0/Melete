-- Users and the bearer tokens they sign in with.
--
-- A token is either a login session (from username + password, one per device) or an API key
-- the user created. Only the SHA-256 of each token is stored, so a copy of this database does
-- not let anyone sign in.

CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- Argon2id, PHC string format.
    password_hash TEXT NOT NULL,
    is_admin INTEGER NOT NULL DEFAULT 0 CHECK (is_admin IN (0, 1)),
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE tokens (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('session', 'api_key')),
    -- Device name for sessions, the user's label for API keys.
    name TEXT NOT NULL,
    token_hash BLOB NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER
) STRICT;

CREATE INDEX tokens_user ON tokens (user_id);
