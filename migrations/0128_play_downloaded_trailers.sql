-- Playing downloaded trailers as extras are played. Each is known to the apps by an id from
-- 2^41 up (its row's id added), in the extras' range, so what holds for an extra holds for it:
-- converted for a player that can't play it, held to a way in's limit, never watched.
--
-- Folders of Melete's own that hold what it plays, by name, written at each start (the trailers
-- folder under the config folder), so a stream link can say where its file is.
CREATE TABLE app_folders (
    name TEXT PRIMARY KEY,
    path TEXT NOT NULL
) STRICT;

-- A downloaded trailer's file as it was when it was taken, and what ffprobe found in it, for that
-- size and date, as media_extras keeps for extras.
ALTER TABLE downloaded_trailers ADD COLUMN size INTEGER CHECK (size IS NULL OR size >= 0);
ALTER TABLE downloaded_trailers ADD COLUMN modified_at INTEGER;
ALTER TABLE downloaded_trailers ADD COLUMN probed_size INTEGER;
ALTER TABLE downloaded_trailers ADD COLUMN probed_modified_at INTEGER;
ALTER TABLE downloaded_trailers ADD COLUMN probe_info TEXT;
ALTER TABLE downloaded_trailers ADD COLUMN probe_error TEXT;

-- Stream links to downloaded trailers, kept apart as extras' are.
CREATE TABLE trailer_play_sessions (
    id INTEGER PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    token_id INTEGER NOT NULL REFERENCES tokens (id) ON DELETE CASCADE,
    trailer_id INTEGER NOT NULL REFERENCES downloaded_trailers (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    -- The conversion's settings, as JSON, for a link to an HLS playlist.
    transcode TEXT
) STRICT;
CREATE INDEX trailer_play_sessions_expiry ON trailer_play_sessions (expires_at);
