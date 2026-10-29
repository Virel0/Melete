-- no-transaction
-- Wanted songs: a single track to have, fetched through Soulseek, where one
-- file is a result. A search can now be for a song, so its table is rebuilt to widen the check
-- that a search has exactly one target, with foreign keys off as in 0033: release_decisions
-- points at searches with ON DELETE CASCADE.

CREATE TABLE wanted_songs (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL CHECK (trim(title) != ''),
    artist TEXT NOT NULL CHECK (trim(artist) != ''),
    -- The album it's on, for its folder and tags; NULL when it's wanted on its own.
    album TEXT CHECK (album IS NULL OR trim(album) != ''),
    -- Its length, which tells a radio edit or a live take from it; NULL when not known.
    length_seconds INTEGER CHECK (length_seconds IS NULL OR length_seconds > 0),
    mb_recording_id TEXT UNIQUE,
    mb_artist_id TEXT,
    -- Where it goes, and the profile its results are decided with.
    library_id INTEGER REFERENCES libraries (id) ON DELETE SET NULL,
    music_profile_id INTEGER REFERENCES music_profiles (id) ON DELETE SET NULL,
    source TEXT NOT NULL CHECK (source IN ('manual')),
    added_at INTEGER NOT NULL
) STRICT;

PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE searches_new (
    id INTEGER PRIMARY KEY,
    wanted_movie_id INTEGER REFERENCES wanted_movies (id) ON DELETE CASCADE,
    wanted_series_id INTEGER REFERENCES wanted_series (id) ON DELETE CASCADE,
    wanted_album_id INTEGER REFERENCES wanted_albums (id) ON DELETE CASCADE,
    wanted_song_id INTEGER REFERENCES wanted_songs (id) ON DELETE CASCADE,
    -- For series: the season searched, and the episode unless it was a season search.
    season INTEGER,
    episode INTEGER,
    profile_id INTEGER REFERENCES quality_profiles (id) ON DELETE SET NULL,
    -- An album's or a song's profile, which isn't a video one.
    music_profile_id INTEGER REFERENCES music_profiles (id) ON DELETE SET NULL,
    searched_at INTEGER NOT NULL,
    -- Each indexer asked: [{"indexer_id", "name", "results", "error"}].
    indexers TEXT NOT NULL CHECK (json_valid(indexers)),
    rss_sync_id INTEGER REFERENCES rss_syncs (id) ON DELETE CASCADE,
    -- How asking slskd went: {"results", "error"}; NULL when it wasn't asked.
    soulseek TEXT CHECK (soulseek IS NULL OR json_valid(soulseek)),
    CHECK ((wanted_movie_id IS NOT NULL) + (wanted_series_id IS NOT NULL)
           + (wanted_album_id IS NOT NULL) + (wanted_song_id IS NOT NULL) = 1),
    CHECK (wanted_series_id IS NOT NULL OR (season IS NULL AND episode IS NULL)),
    CHECK (episode IS NULL OR season IS NOT NULL),
    CHECK (wanted_album_id IS NOT NULL OR wanted_song_id IS NOT NULL OR music_profile_id IS NULL),
    CHECK ((wanted_album_id IS NULL AND wanted_song_id IS NULL) OR profile_id IS NULL)
) STRICT;
INSERT INTO searches_new (id, wanted_movie_id, wanted_series_id, wanted_album_id, season,
    episode, profile_id, music_profile_id, searched_at, indexers, rss_sync_id, soulseek)
SELECT id, wanted_movie_id, wanted_series_id, wanted_album_id, season, episode, profile_id,
    music_profile_id, searched_at, indexers, rss_sync_id, soulseek
FROM searches;
DROP TABLE searches;
ALTER TABLE searches_new RENAME TO searches;
CREATE INDEX searches_by_movie ON searches (wanted_movie_id);
CREATE INDEX searches_by_series ON searches (wanted_series_id);
CREATE INDEX searches_by_album ON searches (wanted_album_id);
CREATE INDEX searches_by_song ON searches (wanted_song_id);
COMMIT;
PRAGMA foreign_keys = ON;
