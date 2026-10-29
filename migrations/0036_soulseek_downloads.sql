-- no-transaction
-- Downloads through slskd: a download is a torrent on the seedbox, tracked by
-- its hash, or a Soulseek user's album folder, tracked by slskd's batches. A Soulseek download
-- has no hash, so the table that required one is rebuilt, with 0034's triggers recreated.
--
-- Rebuilt with foreign keys off, as 0033 did: trashed_files points at downloads.
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE downloads_new (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL DEFAULT 'torrent' CHECK (source IN ('torrent', 'soulseek')),
    -- A torrent's hash; a hash is never grabbed twice.
    infohash TEXT UNIQUE
        CHECK (infohash IS NULL OR (length(infohash) = 40 AND infohash NOT GLOB '*[^0-9a-f]*')),
    -- A Soulseek download's peer, album folder and parts: {"username", "folder", "parts":
    -- [{"batch": "<uuid>", "folder": "" or "CD1", "files": [{"filename", "size"}]}]}. Part n
    -- (from 1) goes into melete/<id>/<n> in slskd's downloads folder.
    soulseek TEXT CHECK (soulseek IS NULL OR json_valid(soulseek)),
    title TEXT NOT NULL,
    wanted_movie_id INTEGER REFERENCES wanted_movies (id) ON DELETE SET NULL,
    wanted_series_id INTEGER REFERENCES wanted_series (id) ON DELETE SET NULL,
    wanted_album_id INTEGER REFERENCES wanted_albums (id) ON DELETE SET NULL,
    season INTEGER,
    episode INTEGER,
    -- The decision that grabbed it, as it was (searches are removed after 30 days).
    decision TEXT NOT NULL CHECK (json_valid(decision)),
    -- downloading: on the seedbox or with slskd; completed: all of it is there; transferring:
    -- being copied here; imported: in the library; failed: see message.
    state TEXT NOT NULL
        CHECK (state IN ('downloading', 'completed', 'transferring', 'imported', 'failed')),
    progress REAL NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 100),
    size INTEGER,
    -- Where Deluge keeps a torrent: its save path, the torrent's name, and its files as
    -- [{"path", "size"}] relative to the save path.
    save_path TEXT,
    name TEXT,
    files TEXT CHECK (files IS NULL OR json_valid(files)),
    -- Deluge's, slskd's or Melete's latest word on it.
    message TEXT,
    -- Since when Deluge hasn't listed it; after half an hour it counts as removed.
    missing_since INTEGER,
    added_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at INTEGER,
    destination TEXT,
    placed TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(placed)),
    replaces TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(replaces)),
    retired_at INTEGER,
    CHECK ((source = 'torrent') = (infohash IS NOT NULL)),
    CHECK ((source = 'soulseek') = (soulseek IS NOT NULL)),
    -- Soulseek gets albums (and later songs), not videos.
    CHECK (source = 'torrent' OR (wanted_movie_id IS NULL AND wanted_series_id IS NULL)),
    CHECK (wanted_movie_id IS NULL OR wanted_series_id IS NULL)
) STRICT;
INSERT INTO downloads_new (id, infohash, title, wanted_movie_id, wanted_series_id,
    wanted_album_id, season, episode, decision, state, progress, size, save_path, name, files,
    message, missing_since, added_at, updated_at, completed_at, attempts, next_attempt_at,
    destination, placed, replaces, retired_at)
SELECT id, infohash, title, wanted_movie_id, wanted_series_id, wanted_album_id, season, episode,
    decision, state, progress, size, save_path, name, files, message, missing_since, added_at,
    updated_at, completed_at, attempts, next_attempt_at, destination, placed, replaces, retired_at
FROM downloads;
DROP TABLE downloads;
ALTER TABLE downloads_new RENAME TO downloads;
CREATE INDEX downloads_by_state ON downloads (state);
-- A Soulseek folder isn't grabbed twice while a download of it is under way or done; after one
-- fails, it can be tried again.
CREATE UNIQUE INDEX downloads_soulseek_folder
    ON downloads (json_extract(soulseek, '$.username'), json_extract(soulseek, '$.folder'))
    WHERE source = 'soulseek' AND state != 'failed';
CREATE TRIGGER downloads_one_title_insert BEFORE INSERT ON downloads
WHEN NEW.wanted_album_id IS NOT NULL
     AND (NEW.wanted_movie_id IS NOT NULL OR NEW.wanted_series_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'a download is for one wanted title');
END;
CREATE TRIGGER downloads_one_title_update BEFORE UPDATE ON downloads
WHEN NEW.wanted_album_id IS NOT NULL
     AND (NEW.wanted_movie_id IS NOT NULL OR NEW.wanted_series_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'a download is for one wanted title');
END;
COMMIT;
PRAGMA foreign_keys = ON;
