-- What the household wants: movies and series to have, on disk or not yet. Imported from Radarr
-- and Sonarr for now; the download side works from these.

CREATE TABLE wanted_movies (
    id INTEGER PRIMARY KEY,
    tmdb_id INTEGER NOT NULL UNIQUE,
    imdb_id TEXT,
    title TEXT NOT NULL,
    year INTEGER,
    monitored INTEGER NOT NULL CHECK (monitored IN (0, 1)),
    -- The profile's name as the *arr called it; Melete's own quality profiles come later.
    quality_profile TEXT,
    -- announced, inCinemas, released or tba, as in Radarr.
    minimum_availability TEXT,
    -- Where new files go; NULL when no library matched the *arr's root folder.
    library_id INTEGER REFERENCES libraries (id),
    -- The adopted folder, when the movie is already on disk.
    movie_id INTEGER REFERENCES movies (id),
    source TEXT NOT NULL CHECK (source IN ('radarr', 'manual')),
    -- The movie's id in the source, for later re-imports.
    source_id INTEGER,
    added_at INTEGER NOT NULL
) STRICT;

CREATE TABLE wanted_series (
    id INTEGER PRIMARY KEY,
    tvdb_id INTEGER NOT NULL UNIQUE,
    tmdb_id INTEGER,
    imdb_id TEXT,
    title TEXT NOT NULL,
    year INTEGER,
    monitored INTEGER NOT NULL CHECK (monitored IN (0, 1)),
    series_type TEXT NOT NULL CHECK (series_type IN ('standard', 'daily', 'anime')),
    quality_profile TEXT,
    library_id INTEGER REFERENCES libraries (id),
    series_id INTEGER REFERENCES series (id),
    source TEXT NOT NULL CHECK (source IN ('sonarr', 'manual')),
    source_id INTEGER,
    added_at INTEGER NOT NULL
) STRICT;

CREATE TABLE wanted_seasons (
    wanted_series_id INTEGER NOT NULL REFERENCES wanted_series (id) ON DELETE CASCADE,
    season INTEGER NOT NULL,
    monitored INTEGER NOT NULL CHECK (monitored IN (0, 1)),
    PRIMARY KEY (wanted_series_id, season)
) STRICT;

-- Episodes unmonitored inside a monitored season (Sonarr allows per-episode choices).
CREATE TABLE wanted_episode_overrides (
    wanted_series_id INTEGER NOT NULL REFERENCES wanted_series (id) ON DELETE CASCADE,
    season INTEGER NOT NULL,
    episode INTEGER NOT NULL,
    monitored INTEGER NOT NULL CHECK (monitored IN (0, 1)),
    PRIMARY KEY (wanted_series_id, season, episode)
) STRICT;

-- Ids the *arrs know for adopted folders. Kept apart from the ids parsed from folder names,
-- which every scan rewrites; matching prefers these.
ALTER TABLE movies ADD COLUMN arr_tmdb_id INTEGER;
ALTER TABLE series ADD COLUMN arr_tvdb_id INTEGER;
ALTER TABLE series ADD COLUMN arr_tmdb_id INTEGER;
