-- MusicBrainz matching: releases fetched from MusicBrainz are cached with their
-- tracks, and each album records which release (and release group) it is, how that was
-- decided, and the candidates when it couldn't be.
CREATE TABLE mb_releases (
    id TEXT PRIMARY KEY,
    release_group_id TEXT NOT NULL,
    title TEXT NOT NULL,
    -- YYYY, YYYY-MM or YYYY-MM-DD, as MusicBrainz gives it.
    date TEXT,
    status TEXT,
    country TEXT,
    -- The release group's: Album, Single, EP…; secondary types (Compilation, Live…) as JSON.
    primary_type TEXT,
    secondary_types TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(secondary_types)),
    first_release_date TEXT,
    -- [{"name", "join", "id", "sort_name"}], in order.
    artist_credit TEXT NOT NULL CHECK (json_valid(artist_credit)),
    -- Whether the Cover Art Archive has a front image for it.
    cover_front INTEGER NOT NULL DEFAULT 0 CHECK (cover_front IN (0, 1)),
    fetched_at INTEGER NOT NULL
) STRICT;

CREATE TABLE mb_release_tracks (
    release_id TEXT NOT NULL REFERENCES mb_releases (id) ON DELETE CASCADE,
    -- The medium's position (disc 1, 2…) and the track's within it.
    disc INTEGER NOT NULL,
    position INTEGER NOT NULL,
    -- As printed: "3", "A1".
    number TEXT,
    title TEXT NOT NULL,
    length_ms INTEGER,
    track_id TEXT NOT NULL,
    recording_id TEXT NOT NULL,
    PRIMARY KEY (release_id, disc, position)
) STRICT;

-- matched by the tags' release id, by a search, or by hand; unmatched; or ambiguous between the
-- release groups in match_candidates. NULL until tried.
ALTER TABLE music_albums ADD COLUMN match_status TEXT
    CHECK (match_status IN ('matched', 'unmatched', 'ambiguous'));
ALTER TABLE music_albums ADD COLUMN match_method TEXT
    CHECK (match_method IN ('tags', 'search', 'manual'));
ALTER TABLE music_albums ADD COLUMN match_release_id TEXT REFERENCES mb_releases (id);
ALTER TABLE music_albums ADD COLUMN match_release_group_id TEXT;
-- [{"release_group_id", "title", "artist", "year", "primary_type", "tracks"}]
ALTER TABLE music_albums ADD COLUMN match_candidates TEXT CHECK (json_valid(match_candidates));
ALTER TABLE music_albums ADD COLUMN matched_at INTEGER;
-- A track's ids from its album's matched release, by disc and position. Kept apart from the
-- ids in its tags (which scans own), and cleared when the file changes, to be linked again.
ALTER TABLE music_tracks ADD COLUMN match_recording_id TEXT;
ALTER TABLE music_tracks ADD COLUMN match_track_id TEXT;
