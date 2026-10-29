-- TMDB metadata, and which TMDB entry each adopted movie or series is.
--
-- The tmdb_* tables are a cache of TMDB's data keyed by TMDB id; several folders may match
-- the same entry. The tmdb_id/tvdb_id/imdb_id columns on movies and series stay what they
-- were: hints parsed from folder names.

CREATE TABLE tmdb_movies (
    tmdb_id INTEGER PRIMARY KEY,
    imdb_id TEXT,
    title TEXT NOT NULL,
    original_title TEXT,
    release_date TEXT,
    year INTEGER,
    overview TEXT,
    runtime_minutes INTEGER,
    poster_path TEXT,
    backdrop_path TEXT,
    -- JSON array of genre names.
    genres TEXT NOT NULL DEFAULT '[]',
    fetched_at INTEGER NOT NULL
) STRICT;

CREATE TABLE tmdb_series (
    tmdb_id INTEGER PRIMARY KEY,
    tvdb_id INTEGER,
    imdb_id TEXT,
    name TEXT NOT NULL,
    original_name TEXT,
    first_air_date TEXT,
    year INTEGER,
    overview TEXT,
    status TEXT,
    poster_path TEXT,
    backdrop_path TEXT,
    genres TEXT NOT NULL DEFAULT '[]',
    fetched_at INTEGER NOT NULL
) STRICT;

CREATE TABLE tmdb_episodes (
    series_tmdb_id INTEGER NOT NULL REFERENCES tmdb_series (tmdb_id) ON DELETE CASCADE,
    season INTEGER NOT NULL,
    episode INTEGER NOT NULL,
    tmdb_id INTEGER NOT NULL,
    name TEXT,
    overview TEXT,
    air_date TEXT,
    runtime_minutes INTEGER,
    still_path TEXT,
    PRIMARY KEY (series_tmdb_id, season, episode)
) STRICT;

-- match_status: NULL = not tried yet (or the last try hit an error),
-- 'matched', 'unmatched' (TMDB had nothing that fits), 'ambiguous' (several fit equally).
-- match_candidates: JSON array of {tmdb_id, title, year} for ambiguous matches.
ALTER TABLE movies ADD COLUMN match_tmdb_id INTEGER REFERENCES tmdb_movies (tmdb_id);
ALTER TABLE movies ADD COLUMN match_status TEXT
    CHECK (match_status IN ('matched', 'unmatched', 'ambiguous'));
ALTER TABLE movies ADD COLUMN match_method TEXT;
ALTER TABLE movies ADD COLUMN match_candidates TEXT;
ALTER TABLE movies ADD COLUMN match_attempted_at INTEGER;

ALTER TABLE series ADD COLUMN match_tmdb_id INTEGER REFERENCES tmdb_series (tmdb_id);
ALTER TABLE series ADD COLUMN match_status TEXT
    CHECK (match_status IN ('matched', 'unmatched', 'ambiguous'));
ALTER TABLE series ADD COLUMN match_method TEXT;
ALTER TABLE series ADD COLUMN match_candidates TEXT;
ALTER TABLE series ADD COLUMN match_attempted_at INTEGER;
