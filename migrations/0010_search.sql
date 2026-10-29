-- Searching indexers for wanted titles: every result is decided on and kept with
-- its reasons (decision 9).

-- The title's original language (ISO 639-1), from the *arr; a release name that gives no
-- language is assumed to be in it.
ALTER TABLE wanted_movies ADD COLUMN original_language TEXT;
ALTER TABLE wanted_series ADD COLUMN original_language TEXT;

CREATE TABLE searches (
    id INTEGER PRIMARY KEY,
    wanted_movie_id INTEGER REFERENCES wanted_movies (id) ON DELETE CASCADE,
    wanted_series_id INTEGER REFERENCES wanted_series (id) ON DELETE CASCADE,
    -- For series: the season searched, and the episode unless it was a season search.
    season INTEGER,
    episode INTEGER,
    profile_id INTEGER REFERENCES quality_profiles (id) ON DELETE SET NULL,
    searched_at INTEGER NOT NULL,
    -- Each indexer asked: [{"indexer_id", "name", "results", "error"}].
    indexers TEXT NOT NULL CHECK (json_valid(indexers)),
    CHECK ((wanted_movie_id IS NULL) <> (wanted_series_id IS NULL)),
    CHECK (wanted_series_id IS NOT NULL OR (season IS NULL AND episode IS NULL)),
    CHECK (episode IS NULL OR season IS NOT NULL)
) STRICT;

CREATE TABLE release_decisions (
    id INTEGER PRIMARY KEY,
    search_id INTEGER NOT NULL REFERENCES searches (id) ON DELETE CASCADE,
    indexer_id INTEGER REFERENCES indexers (id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    size INTEGER,
    seeders INTEGER,
    peers INTEGER,
    published INTEGER,
    infohash TEXT,
    -- What a download grabs. Both can carry the indexer's key or a tracker passkey: never shown.
    download_url TEXT,
    guid TEXT,
    accepted INTEGER NOT NULL CHECK (accepted IN (0, 1)),
    -- The decision with its quality, formats, score and reasons (src/profiles/decide.rs).
    decision TEXT NOT NULL CHECK (json_valid(decision))
) STRICT;

CREATE INDEX release_decisions_by_search ON release_decisions (search_id);
CREATE INDEX searches_by_movie ON searches (wanted_movie_id);
CREATE INDEX searches_by_series ON searches (wanted_series_id);
