-- Guests of a watch party: someone with no account, let in by a link a member made, to that
-- one party and nothing else. Each link lets one guest in once; the pass it gives ends with the
-- party, or when they leave or are sent away.
CREATE TABLE watch_party_invites (
    id INTEGER PRIMARY KEY,
    party_id INTEGER NOT NULL REFERENCES watch_parties (id) ON DELETE CASCADE,
    -- The link's secret, hashed; the link itself is shown once, to who made it.
    invite_hash BLOB NOT NULL UNIQUE,
    created_by INTEGER REFERENCES users (id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE TABLE watch_party_guests (
    id INTEGER PRIMARY KEY,
    party_id INTEGER NOT NULL REFERENCES watch_parties (id) ON DELETE CASCADE,
    -- What they called themselves, shown to the party.
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 40),
    -- Their pass, hashed: kept in their browser as a cookie for the party's routes alone.
    pass_hash BLOB NOT NULL UNIQUE,
    invited_by INTEGER REFERENCES users (id) ON DELETE SET NULL,
    joined_at INTEGER NOT NULL
) STRICT;

-- A guest's stream links, for the file the party plays: they end with the guest.
CREATE TABLE guest_play_sessions (
    id INTEGER PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    guest_id INTEGER NOT NULL REFERENCES watch_party_guests (id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
    transcode TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;
CREATE INDEX guest_play_sessions_expiry ON guest_play_sessions (expires_at);
