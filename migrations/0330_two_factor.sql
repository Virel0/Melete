-- Two-factor sign-in: a code from an authenticator app (TOTP), asked for after a right password.
--
-- The secret is kept as it is, as the settings' secrets are: checking a code needs it, so it
-- can't be hashed the way tokens are. It's never sent again once setup is confirmed.
CREATE TABLE two_factor (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- 20 random bytes (160 bits), the length RFC 4226 recommends.
    secret BLOB NOT NULL CHECK (length(secret) = 20),
    created_at INTEGER NOT NULL,
    -- NULL while setup waits for its first code; sign-in asks for a code only once it's set.
    confirmed_at INTEGER,
    -- The newest 30-second step a code was accepted for; a code is taken once per step, and
    -- never for a step older than this.
    last_step INTEGER
) STRICT;

-- A sign-in waiting for its code, after its password was right. Only the SHA-256 of the
-- challenge is kept; it lasts five minutes and signs in once.
CREATE TABLE sign_in_challenges (
    challenge_hash BLOB PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device_name TEXT NOT NULL CHECK (device_name != '' AND length(device_name) <= 400),
    -- Started by the web UI's sign-in (finished to a cookie) rather than an app's.
    for_web INTEGER NOT NULL CHECK (for_web IN (0, 1)),
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX sign_in_challenges_expiry ON sign_in_challenges (expires_at);

-- Changes to how a user signs in, for their own record: turning the second factor on or off,
-- from their account or from the server's command line.
CREATE TABLE account_changes (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    change TEXT NOT NULL CHECK (change IN ('two_factor_on', 'two_factor_off')),
    made_from TEXT NOT NULL CHECK (made_from IN ('account', 'server')),
    at INTEGER NOT NULL
) STRICT;

CREATE INDEX account_changes_user ON account_changes (user_id, at);
