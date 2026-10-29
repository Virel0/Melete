-- Import lists: charts and lists of movies or shows from TMDB, IMDb or Rotten Tomatoes,
-- whose titles are added to the wanted list on a schedule.
CREATE TABLE import_lists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL CHECK (provider IN ('tmdb', 'imdb', 'rottentomatoes')),
    -- Which list: for tmdb "popular", "top_rated", "trending_day", "trending_week",
    -- "now_playing", "upcoming", "on_the_air", "airing_today" or "list:<id>"; for imdb "top",
    -- "popular" or a public list "ls<digits>"; for rottentomatoes a browse page's URL.
    list TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('movies', 'series')),
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    monitored INTEGER NOT NULL CHECK (monitored IN (0, 1)),
    -- A profile's name for this kind of title, as wanted titles name theirs.
    quality_profile TEXT NOT NULL,
    max_items INTEGER NOT NULL CHECK (max_items BETWEEN 1 AND 250),
    -- For series: which seasons a new title monitors.
    monitor_seasons TEXT NOT NULL DEFAULT 'latest'
        CHECK (monitor_seasons IN ('all', 'latest', 'none')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    added_at INTEGER NOT NULL,
    synced_at INTEGER,
    -- The last sync: {"found", "added", "already_wanted", "skipped": [{"title", "why"}]}, or
    -- {"error"}.
    last_result TEXT CHECK (last_result IS NULL OR json_valid(last_result))
) STRICT;

-- The list a wanted title came from (source 'manual': lists are Melete's own).
ALTER TABLE wanted_movies ADD COLUMN list_id INTEGER REFERENCES import_lists (id) ON DELETE SET NULL;
ALTER TABLE wanted_series ADD COLUMN list_id INTEGER REFERENCES import_lists (id) ON DELETE SET NULL;
