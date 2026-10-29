-- Sharing what you listen to with the rest of the household: each user's own choice,
-- off unless they turn it on. A user who has never chosen has no row.
CREATE TABLE music_listening_sharing (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    sharing INTEGER NOT NULL CHECK (sharing IN (0, 1)),
    -- Unix seconds.
    changed_at INTEGER NOT NULL
) STRICT;
