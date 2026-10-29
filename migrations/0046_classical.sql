-- Classical music: composers, works and their parts (movements).

-- What a track's tags say. `tags_version` is which reading of tags a row has, so fields added
-- later are read from files scanned before, without the files changing.
ALTER TABLE music_tracks ADD COLUMN composer TEXT;
ALTER TABLE music_tracks ADD COLUMN work TEXT;
ALTER TABLE music_tracks ADD COLUMN movement TEXT;
ALTER TABLE music_tracks ADD COLUMN movement_number INTEGER CHECK (movement_number > 0);
ALTER TABLE music_tracks ADD COLUMN movement_total INTEGER CHECK (movement_total > 0);
ALTER TABLE music_tracks ADD COLUMN tags_version INTEGER NOT NULL DEFAULT 0;

-- What MusicBrainz says each release track performs: the work (the whole work, when the track
-- is one of its parts), the part and its number, and the work's composers
-- ([{"id", "name", "sort_name"}]).
ALTER TABLE mb_release_tracks ADD COLUMN work_id TEXT;
ALTER TABLE mb_release_tracks ADD COLUMN work_title TEXT;
ALTER TABLE mb_release_tracks ADD COLUMN movement TEXT;
ALTER TABLE mb_release_tracks ADD COLUMN movement_number INTEGER;
ALTER TABLE mb_release_tracks ADD COLUMN composers TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(composers));
-- Whether the release was fetched with its works; ones fetched before are fetched again.
ALTER TABLE mb_releases ADD COLUMN with_works INTEGER NOT NULL DEFAULT 0
    CHECK (with_works IN (0, 1));

-- Composers and works for browsing, worked out from the tracks after each scan and match. Keyed
-- by MusicBrainz id where known ("mb:<id>"), else by name ("name:<folded>").
CREATE TABLE music_composers (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    sort_name TEXT,
    mb_artist_id TEXT
) STRICT;

CREATE TABLE music_works (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    composer_id INTEGER REFERENCES music_composers (id),
    mb_work_id TEXT
) STRICT;
CREATE INDEX music_works_by_composer ON music_works (composer_id);

-- Each present track's composer, work and part, tags first, MusicBrainz filling the gaps.
CREATE TABLE music_track_works (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    composer_id INTEGER REFERENCES music_composers (id),
    work_id INTEGER REFERENCES music_works (id),
    movement TEXT,
    movement_number INTEGER CHECK (movement_number > 0),
    CHECK (composer_id IS NOT NULL OR work_id IS NOT NULL)
) STRICT;
CREATE INDEX music_track_works_by_composer ON music_track_works (composer_id);
CREATE INDEX music_track_works_by_work ON music_track_works (work_id);
