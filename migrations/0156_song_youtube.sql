-- Songs fetched from YouTube when Soulseek had none (MELETE_SONG_YOUTUBE, off by default), by the
-- trailer downloader: one row per wanted song, for its latest try.
CREATE TABLE song_youtube (
    id INTEGER PRIMARY KEY,
    wanted_song_id INTEGER NOT NULL UNIQUE REFERENCES wanted_songs (id) ON DELETE CASCADE,
    -- searching: a search job is with the downloader; downloading: the video chosen is being
    -- fetched; imported: in the library at `placed`; none: no video fitted (see message);
    -- failed: see message, tried again a day later, three times at most.
    state TEXT NOT NULL
        CHECK (state IN ('searching', 'downloading', 'imported', 'none', 'failed')),
    -- The video chosen: its id, title, channel and length in seconds.
    video TEXT CHECK (video IS NULL OR (length(video) = 11 AND video NOT GLOB '*[^A-Za-z0-9_-]*')),
    video_title TEXT,
    channel TEXT,
    duration_seconds REAL,
    -- The file put in the library, from the library's root.
    placed TEXT,
    message TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (state <> 'downloading' OR video IS NOT NULL),
    CHECK (state <> 'imported' OR (video IS NOT NULL AND placed IS NOT NULL))
) STRICT;
