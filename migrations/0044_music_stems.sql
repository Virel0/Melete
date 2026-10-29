-- Music tracks split into their parts: the voice in full, drums, bass, guitar and other
-- for the first and last minute or so, made by the separator beside Melete. One row per track,
-- for the file as it was when it was separated: a changed file is separated again.
CREATE TABLE music_stems (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('queued', 'done', 'failed')),
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    -- Its parts' folder, under the stems folder.
    folder TEXT NOT NULL,
    -- What the separator wrote about them (its stems.json), once done.
    manifest TEXT CHECK (manifest IS NULL OR json_valid(manifest)),
    error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    queued_at INTEGER NOT NULL,
    finished_at INTEGER,
    CHECK ((state = 'done') = (manifest IS NOT NULL)),
    CHECK ((state = 'failed') = (error IS NOT NULL)),
    CHECK ((state = 'queued') = (finished_at IS NULL))
) STRICT;
CREATE INDEX music_stems_by_state ON music_stems (state);
