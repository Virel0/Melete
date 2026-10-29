-- Extras: trailers, featurettes and other bonus videos in a title's folder, kept apart from
-- media_files so they never count as the movie or as episodes (nor in upgrades, searches or
-- Next Up). Their ids start at 2^40, so an extra's id is never a library file's and both play
-- through the same routes. Like files, a vanished extra gets missing_since and is never deleted.
CREATE TABLE media_extras (
    id INTEGER PRIMARY KEY CHECK (id >= 1099511627776),
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    rel_path TEXT NOT NULL,
    size INTEGER NOT NULL CHECK (size >= 0),
    modified_at INTEGER NOT NULL,
    movie_id INTEGER REFERENCES movies (id),
    series_id INTEGER REFERENCES series (id),
    kind TEXT NOT NULL CHECK (kind IN ('trailer', 'featurette', 'behind_the_scenes',
        'deleted_scene', 'interview', 'scene', 'short', 'clip', 'other')),
    -- From the file's name.
    title TEXT NOT NULL,
    first_seen_generation INTEGER NOT NULL,
    seen_generation INTEGER NOT NULL,
    missing_since INTEGER,
    -- What ffprobe found, for the size and date it looked at (as media_probes keeps for files).
    probed_size INTEGER,
    probed_modified_at INTEGER,
    probe_info TEXT,
    probe_error TEXT,
    UNIQUE (library_id, rel_path),
    CHECK ((movie_id IS NULL) <> (series_id IS NULL))
) STRICT;

CREATE INDEX media_extras_movie ON media_extras (movie_id) WHERE movie_id IS NOT NULL;
CREATE INDEX media_extras_series ON media_extras (series_id) WHERE series_id IS NOT NULL;

-- Stream links for extras, as play_sessions are for files (whose file_id must be a file's).
CREATE TABLE extra_play_sessions (
    id INTEGER PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    token_id INTEGER NOT NULL REFERENCES tokens (id) ON DELETE CASCADE,
    extra_id INTEGER NOT NULL REFERENCES media_extras (id),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    -- The conversion's settings, as JSON, for a link to an HLS playlist.
    transcode TEXT
) STRICT;

CREATE INDEX extra_play_sessions_expiry ON extra_play_sessions (expires_at);
