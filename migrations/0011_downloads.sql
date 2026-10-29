-- Downloads: releases sent to Deluge on the seedbox, tracked by torrent hash. A hash
-- is never grabbed twice; this table, not what's in a folder, says what was already fetched.

CREATE TABLE downloads (
    id INTEGER PRIMARY KEY,
    infohash TEXT NOT NULL UNIQUE
        CHECK (length(infohash) = 40 AND infohash NOT GLOB '*[^0-9a-f]*'),
    title TEXT NOT NULL,
    wanted_movie_id INTEGER REFERENCES wanted_movies (id) ON DELETE SET NULL,
    wanted_series_id INTEGER REFERENCES wanted_series (id) ON DELETE SET NULL,
    season INTEGER,
    episode INTEGER,
    -- The decision that grabbed it, as it was (searches are removed after 30 days).
    decision TEXT NOT NULL CHECK (json_valid(decision)),
    -- downloading: on the seedbox; completed: Deluge has all of it; transferring: being
    -- copied here; imported: in the library; failed: see message.
    state TEXT NOT NULL
        CHECK (state IN ('downloading', 'completed', 'transferring', 'imported', 'failed')),
    progress REAL NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 100),
    size INTEGER,
    -- Where Deluge keeps it: its save path, the torrent's name, and its files as
    -- [{"path", "size"}] relative to the save path.
    save_path TEXT,
    name TEXT,
    files TEXT CHECK (files IS NULL OR json_valid(files)),
    -- Deluge's or Melete's latest word on it.
    message TEXT,
    -- Since when Deluge hasn't listed it; after half an hour it counts as removed.
    missing_since INTEGER,
    added_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    CHECK (wanted_movie_id IS NULL OR wanted_series_id IS NULL)
) STRICT;

CREATE INDEX downloads_by_state ON downloads (state);
