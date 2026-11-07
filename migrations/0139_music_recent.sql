-- What each listener started lately, for Home's Jump back in: albums, playlists and stations
-- (radio from a song). Removing one hides it until it's started again. Each user's own; at most
-- the latest 50 are kept.
CREATE TABLE music_recent (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('album', 'playlist', 'station')),
    -- An album's id, an Aoide playlist's id, or a station's seed song's id.
    item TEXT NOT NULL CHECK (length(item) BETWEEN 1 AND 200),
    -- Unix milliseconds: when it was last started (or removed, for an album only ever removed).
    played_at INTEGER NOT NULL,
    -- 1 once removed from Jump back in, until it's started again.
    removed INTEGER NOT NULL DEFAULT 0 CHECK (removed IN (0, 1)),
    PRIMARY KEY (user_id, kind, item)
) STRICT;
