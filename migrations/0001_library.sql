-- Libraries and what scans found in them.
--
-- A scan never deletes rows. A file that disappears gets missing_since instead, because a
-- NAS dropping out of a mergerfs pool looks exactly like half the library being deleted.
-- Times are unix seconds.

CREATE TABLE libraries (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('movies', 'shows')),
    path TEXT NOT NULL,
    -- Bumped by every scan; rows not stamped with the latest one were not found by it.
    scan_generation INTEGER NOT NULL DEFAULT 0,
    last_scan_at INTEGER
) STRICT;

-- One top-level folder (or loose file) in a movies library.
CREATE TABLE movies (
    id INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    rel_path TEXT NOT NULL,
    parsed_title TEXT NOT NULL,
    parsed_year INTEGER,
    tmdb_id INTEGER,
    imdb_id TEXT,
    seen_generation INTEGER NOT NULL,
    missing_since INTEGER,
    UNIQUE (library_id, rel_path)
) STRICT;

-- One top-level folder in a shows library.
CREATE TABLE series (
    id INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    rel_path TEXT NOT NULL,
    parsed_title TEXT NOT NULL,
    parsed_year INTEGER,
    tmdb_id INTEGER,
    tvdb_id INTEGER,
    imdb_id TEXT,
    seen_generation INTEGER NOT NULL,
    missing_since INTEGER,
    UNIQUE (library_id, rel_path)
) STRICT;

CREATE TABLE media_files (
    id INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    rel_path TEXT NOT NULL,
    size INTEGER NOT NULL CHECK (size >= 0),
    modified_at INTEGER NOT NULL,
    movie_id INTEGER REFERENCES movies (id),
    series_id INTEGER REFERENCES series (id),
    -- Episode files only; both NULL when the name could not be parsed.
    season INTEGER,
    episode INTEGER,
    -- Last episode of a multi-episode file (S01E01E02, S01E01-E03).
    episode_end INTEGER,
    first_seen_generation INTEGER NOT NULL,
    seen_generation INTEGER NOT NULL,
    missing_since INTEGER,
    UNIQUE (library_id, rel_path),
    CHECK ((movie_id IS NULL) <> (series_id IS NULL)),
    CHECK (movie_id IS NULL OR season IS NULL),
    CHECK ((season IS NULL) = (episode IS NULL)),
    CHECK (episode_end IS NULL OR episode_end > episode)
) STRICT;

CREATE INDEX media_files_movie ON media_files (movie_id);
CREATE INDEX media_files_series ON media_files (series_id, season, episode);
