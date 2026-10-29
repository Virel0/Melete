-- Lyrics on the server: each track's lyrics as last found, so every app shows the same
-- and LRCLIB is asked about a track once. From an .lrc file beside the track, the track's own
-- tags, or LRCLIB; 'none' records that LRCLIB had nothing, to ask again after a while.
CREATE TABLE music_lyrics (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    source TEXT NOT NULL CHECK (source IN ('file', 'tags', 'lrclib', 'none')),
    -- LRC text when synced, plain lines otherwise; none for 'none'.
    text TEXT,
    synced INTEGER NOT NULL CHECK (synced IN (0, 1)),
    -- The track file's size and modification time when found: a changed file is looked at
    -- afresh.
    track_size INTEGER NOT NULL,
    track_modified_at INTEGER NOT NULL,
    -- Unix seconds.
    checked_at INTEGER NOT NULL,
    CHECK ((source = 'none') = (text IS NULL))
) STRICT;
