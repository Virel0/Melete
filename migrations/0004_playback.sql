-- Playback: short-lived stream URLs and per-user watch progress.

-- A stream URL carries its own token so players that can't send headers (AVPlayer, VLC) never
-- see the user's real token. It is tied to the token that created it, so signing out, revoking
-- the device or changing the password ends its streams too. Only the SHA-256 is stored.
CREATE TABLE play_sessions (
    id INTEGER PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    token_id INTEGER NOT NULL REFERENCES tokens (id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES media_files (id),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    -- Set when a play through this link reached the end, so a viewing counts once however
    -- often the player rewinds past the 90% mark. Watching again means asking to play again.
    counted INTEGER NOT NULL DEFAULT 0 CHECK (counted IN (0, 1))
) STRICT;

CREATE INDEX play_sessions_expiry ON play_sessions (expires_at);

-- Progress is per file, so versions of a movie and unparsed episode files each keep their own.
CREATE TABLE playback_progress (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES media_files (id),
    position_seconds REAL NOT NULL CHECK (position_seconds >= 0),
    duration_seconds REAL CHECK (duration_seconds > 0),
    watched INTEGER NOT NULL DEFAULT 0 CHECK (watched IN (0, 1)),
    play_count INTEGER NOT NULL DEFAULT 0 CHECK (play_count >= 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, file_id)
) STRICT;

CREATE INDEX playback_progress_recent ON playback_progress (user_id, updated_at);
