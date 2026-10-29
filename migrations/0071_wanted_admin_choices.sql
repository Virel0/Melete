-- An admin's own choices on the wanted list, which imports from Radarr, Sonarr and Lidarr leave
-- alone, as Melete's own indexer switch is (0067). `monitored_here` says monitoring was
-- set in Melete: an import no longer changes it. `wanted_removed` remembers a title taken off
-- the list in Melete, by the id the import knows it by, so the import doesn't add it back;
-- adding the title again by hand puts it back in the import's hands.
ALTER TABLE wanted_movies ADD COLUMN monitored_here INTEGER NOT NULL DEFAULT 0
    CHECK (monitored_here IN (0, 1));
ALTER TABLE wanted_series ADD COLUMN monitored_here INTEGER NOT NULL DEFAULT 0
    CHECK (monitored_here IN (0, 1));
ALTER TABLE wanted_artists ADD COLUMN monitored_here INTEGER NOT NULL DEFAULT 0
    CHECK (monitored_here IN (0, 1));
ALTER TABLE wanted_albums ADD COLUMN monitored_here INTEGER NOT NULL DEFAULT 0
    CHECK (monitored_here IN (0, 1));

CREATE TABLE wanted_removed (
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'series', 'artist', 'album')),
    -- A movie's TMDB id, a series' TVDB id, an artist's or album's MusicBrainz id.
    key TEXT NOT NULL CHECK (key != ''),
    title TEXT NOT NULL,
    removed_at INTEGER NOT NULL,
    PRIMARY KEY (kind, key)
) STRICT;
