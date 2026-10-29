-- no-transaction
-- Searching for wanted albums: a search can be for a wanted album, decided with
-- a music profile, and its results join the same release decisions as video's.
--
-- Widening the check means rebuilding the table, safe in SQLite only with foreign keys off (its
-- 12-step procedure): with them on, dropping the old table would fire release_decisions'
-- ON DELETE CASCADE. Hence no transaction around the migration, and one of its own around the
-- rebuild.
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE searches_new (
    id INTEGER PRIMARY KEY,
    wanted_movie_id INTEGER REFERENCES wanted_movies (id) ON DELETE CASCADE,
    wanted_series_id INTEGER REFERENCES wanted_series (id) ON DELETE CASCADE,
    wanted_album_id INTEGER REFERENCES wanted_albums (id) ON DELETE CASCADE,
    -- For series: the season searched, and the episode unless it was a season search.
    season INTEGER,
    episode INTEGER,
    profile_id INTEGER REFERENCES quality_profiles (id) ON DELETE SET NULL,
    -- An album's profile, which isn't a video one.
    music_profile_id INTEGER REFERENCES music_profiles (id) ON DELETE SET NULL,
    searched_at INTEGER NOT NULL,
    -- Each indexer asked: [{"indexer_id", "name", "results", "error"}].
    indexers TEXT NOT NULL CHECK (json_valid(indexers)),
    rss_sync_id INTEGER REFERENCES rss_syncs (id) ON DELETE CASCADE,
    CHECK ((wanted_movie_id IS NOT NULL) + (wanted_series_id IS NOT NULL)
           + (wanted_album_id IS NOT NULL) = 1),
    CHECK (wanted_series_id IS NOT NULL OR (season IS NULL AND episode IS NULL)),
    CHECK (episode IS NULL OR season IS NOT NULL),
    CHECK (wanted_album_id IS NOT NULL OR music_profile_id IS NULL),
    CHECK (wanted_album_id IS NULL OR profile_id IS NULL)
) STRICT;
INSERT INTO searches_new (id, wanted_movie_id, wanted_series_id, season, episode, profile_id,
    searched_at, indexers, rss_sync_id)
SELECT id, wanted_movie_id, wanted_series_id, season, episode, profile_id, searched_at,
    indexers, rss_sync_id
FROM searches;
DROP TABLE searches;
ALTER TABLE searches_new RENAME TO searches;
CREATE INDEX searches_by_movie ON searches (wanted_movie_id);
CREATE INDEX searches_by_series ON searches (wanted_series_id);
CREATE INDEX searches_by_album ON searches (wanted_album_id);
COMMIT;
PRAGMA foreign_keys = ON;

-- The Pirate Bay is searched for music too (its Music and FLAC categories).
UPDATE indexers SET categories = '[2000,2040,2045,3000,3040,5000,5040,5045]'
WHERE kind = 'apibay' AND categories = '[2000,2040,2045,5000,5040,5045]';
