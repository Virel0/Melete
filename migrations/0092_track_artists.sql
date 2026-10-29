-- Tracks with several artists: each track's artists in credit order, so a featured artist or a
-- duet partner is found under their own name, not only the album artist.

-- What a track's tags list apart from the artist string, in order: the names of a multi-value
-- artist tag (ARTISTS, or several ARTIST values), and every MusicBrainz artist id. Empty when
-- the tags give one artist. Files scanned before are read again for them (`tags_version`).
ALTER TABLE music_tracks ADD COLUMN artist_names TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(artist_names));
ALTER TABLE music_tracks ADD COLUMN mb_artist_ids TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(mb_artist_ids));

-- What MusicBrainz credits on each release track ([{"name", "join", "id", "artist"}], `name` as
-- credited and `artist` the artist's own name); NULL for releases cached before this was kept.
ALTER TABLE mb_release_tracks ADD COLUMN artist_credit TEXT CHECK (json_valid(artist_credit));

-- Each present track's artists, worked out after each scan and match and rebuilt whole each
-- time. `name` is as credited on the track; the artist row keeps the artist's own.
CREATE TABLE music_track_artists (
    track_id INTEGER NOT NULL REFERENCES music_tracks (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    artist_id INTEGER NOT NULL REFERENCES music_artists (id),
    name TEXT NOT NULL,
    PRIMARY KEY (track_id, position),
    UNIQUE (track_id, artist_id)
) STRICT;
CREATE INDEX music_track_artists_by_artist ON music_track_artists (artist_id, track_id);
