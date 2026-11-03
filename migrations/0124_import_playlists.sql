-- A playlist import can make an Aoide playlist of its tracks: its id in the importer's sync
-- log, NULL when it makes none. Its tracks the library has already are put in that playlist as
-- they are, not asked for: a new state, 'in_library', so the track list is rebuilt to widen
-- its check. Nothing points at playlist_import_tracks, so it's dropped and renamed in place.
ALTER TABLE playlist_imports ADD COLUMN playlist_id TEXT;

CREATE TABLE playlist_import_tracks_new (
    import_id INTEGER NOT NULL REFERENCES playlist_imports (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position > 0),
    title TEXT NOT NULL CHECK (trim(title) != ''),
    artist TEXT,
    album TEXT,
    isrc TEXT,
    length_ms INTEGER CHECK (length_ms IS NULL OR length_ms > 0),
    state TEXT NOT NULL
        CHECK (state IN ('pending', 'asked', 'already', 'unmatched', 'in_library')),
    why TEXT,
    request_id INTEGER REFERENCES requests (id) ON DELETE SET NULL,
    PRIMARY KEY (import_id, position),
    CHECK ((state = 'unmatched') = (why IS NOT NULL))
) STRICT;
INSERT INTO playlist_import_tracks_new (import_id, position, title, artist, album, isrc,
    length_ms, state, why, request_id)
SELECT import_id, position, title, artist, album, isrc, length_ms, state, why, request_id
FROM playlist_import_tracks;
DROP TABLE playlist_import_tracks;
ALTER TABLE playlist_import_tracks_new RENAME TO playlist_import_tracks;
CREATE INDEX playlist_import_tracks_by_request ON playlist_import_tracks (request_id);

-- Finding the playlist items that wait on a song request, after every scan, and the tracks
-- such a request brought, without reading the whole sync log or track list each time.
CREATE INDEX aoide_ops_by_pending_request ON aoide_ops (json_extract(payload, '$.pending.requestId'))
    WHERE entity = 'playlist_items' AND json_extract(payload, '$.pending.requestId') IS NOT NULL;
CREATE INDEX aoide_ops_by_item ON aoide_ops (entity_id) WHERE entity = 'playlist_items';
CREATE INDEX music_tracks_by_recording ON music_tracks (coalesce(mb_recording_id, match_recording_id));
