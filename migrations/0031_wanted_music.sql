-- Wanted music: artists and their albums to have, on disk or not yet, keyed by
-- MusicBrainz ids. Imported from Lidarr for now; searching and downloading work from these.
-- Whether an album is on disk is read from the music library (its MusicBrainz release group),
-- not stored, so it can't go stale.
CREATE TABLE wanted_artists (
    id INTEGER PRIMARY KEY,
    mb_artist_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    monitored INTEGER NOT NULL CHECK (monitored IN (0, 1)),
    -- What happens to albums the artist releases later, as in Lidarr.
    monitor_new TEXT NOT NULL CHECK (monitor_new IN ('all', 'new', 'none')),
    -- The profile's name as Lidarr called it; music profiles of Melete's own come next.
    quality_profile TEXT,
    -- Where new albums go; NULL when no music library matched Lidarr's root folder.
    library_id INTEGER REFERENCES libraries (id),
    source TEXT NOT NULL CHECK (source IN ('lidarr', 'manual')),
    -- The artist's id in the source, for later re-imports.
    source_id INTEGER,
    added_at INTEGER NOT NULL
) STRICT;

CREATE TABLE wanted_albums (
    id INTEGER PRIMARY KEY,
    wanted_artist_id INTEGER NOT NULL REFERENCES wanted_artists (id) ON DELETE CASCADE,
    -- The MusicBrainz release group: the album, whichever edition of it arrives.
    mb_release_group_id TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    -- Album, EP, Single… and secondary types (Live, Compilation…) as a JSON list.
    album_type TEXT,
    secondary_types TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(secondary_types)),
    -- YYYY-MM-DD, or shorter when only the year or month is known.
    release_date TEXT,
    monitored INTEGER NOT NULL CHECK (monitored IN (0, 1)),
    source TEXT NOT NULL CHECK (source IN ('lidarr', 'manual')),
    source_id INTEGER,
    added_at INTEGER NOT NULL
) STRICT;
CREATE INDEX wanted_albums_by_artist ON wanted_albums (wanted_artist_id);
