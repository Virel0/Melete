-- no-transaction
-- Music libraries: a third kind of library, and what a scan of one records. Tracks are
-- the files with what their tags say; albums group them (by MusicBrainz release, else by album
-- tag within a folder, else by folder); artists are the albums' artists. Nothing on disk is
-- ever written.
--
-- Widening the kind check means rebuilding libraries, which every other table points at, so
-- foreign keys are off around it (SQLite's 12-step procedure, as 0020 did for indexers): with
-- them on, dropping the old table would fire ON DELETE actions in the tables that point at it.
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE libraries_new (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('movies', 'shows', 'music')),
    path TEXT NOT NULL,
    -- Bumped by every scan; rows not stamped with the latest one were not found by it.
    scan_generation INTEGER NOT NULL DEFAULT 0,
    last_scan_at INTEGER
) STRICT;
INSERT INTO libraries_new (id, name, kind, path, scan_generation, last_scan_at)
SELECT id, name, kind, path, scan_generation, last_scan_at FROM libraries;
DROP TABLE libraries;
ALTER TABLE libraries_new RENAME TO libraries;

-- An album's artist, shared across libraries. Keyed by MusicBrainz id when the tags give one,
-- else by name folded to lower case.
CREATE TABLE music_artists (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    mb_artist_id TEXT
) STRICT;

CREATE TABLE music_albums (
    id INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    -- mb:<release id>, tag:<folder>␟<album tag folded>, or dir:<folder>.
    key TEXT NOT NULL,
    title TEXT NOT NULL,
    -- NULL for a compilation of several artists.
    artist_id INTEGER REFERENCES music_artists (id),
    -- As shown: the album artist, or "Various Artists".
    artist_name TEXT NOT NULL,
    year INTEGER,
    -- The folder its tracks are in (a disc folder's parent), relative to the library.
    folder TEXT NOT NULL,
    compilation INTEGER NOT NULL DEFAULT 0 CHECK (compilation IN (0, 1)),
    mb_release_id TEXT,
    mb_release_group_id TEXT,
    seen_generation INTEGER NOT NULL,
    missing_since INTEGER,
    UNIQUE (library_id, key)
) STRICT;

CREATE TABLE music_tracks (
    id INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    rel_path TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    first_seen_generation INTEGER NOT NULL,
    seen_generation INTEGER NOT NULL,
    missing_since INTEGER,
    album_id INTEGER REFERENCES music_albums (id),
    -- Whether its tags could be read; when not, the fields below come from its name.
    tagged INTEGER NOT NULL CHECK (tagged IN (0, 1)),
    -- Why the file couldn't be read, when it couldn't.
    read_error TEXT,
    title TEXT NOT NULL,
    artist TEXT,
    album_artist TEXT,
    album TEXT,
    track_number INTEGER,
    disc_number INTEGER,
    year INTEGER,
    genres TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(genres)),
    compilation INTEGER NOT NULL DEFAULT 0 CHECK (compilation IN (0, 1)),
    duration_seconds REAL,
    -- flac, mp3, aac, alac, opus, vorbis, wav, aiff, wavpack, ape, musepack, speex.
    codec TEXT,
    bitrate_kbps INTEGER,
    sample_rate INTEGER,
    bit_depth INTEGER,
    channels INTEGER,
    isrc TEXT,
    mb_recording_id TEXT,
    mb_track_id TEXT,
    mb_release_id TEXT,
    mb_release_group_id TEXT,
    mb_artist_id TEXT,
    mb_album_artist_id TEXT,
    UNIQUE (library_id, rel_path)
) STRICT;
CREATE INDEX music_tracks_by_album ON music_tracks (album_id);
CREATE INDEX music_albums_by_artist ON music_albums (artist_id);
COMMIT;
PRAGMA foreign_keys = ON;
