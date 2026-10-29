-- Pinned albums, artists and playlists: up to six per user, in their order, the same
-- on every device.
CREATE TABLE music_pins (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 5),
    kind TEXT NOT NULL CHECK (kind IN ('album', 'artist', 'playlist')),
    -- An album's or artist's id, or an Aoide playlist's.
    item TEXT NOT NULL,
    PRIMARY KEY (user_id, position),
    UNIQUE (user_id, kind, item)
) STRICT;

-- Each user's pins as a whole: a version that goes up with every change, for the apps to
-- tell theirs is out of date. A user who has never pinned anything has no row.
CREATE TABLE music_pin_lists (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    -- Unix seconds.
    updated_at INTEGER NOT NULL
) STRICT;
