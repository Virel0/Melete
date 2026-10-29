-- Playlists imported as song requests: an import, and each of its tracks with what
-- became of it. A track is pending until matched; then asked for (its request), asked for
-- already by the same user (that request), or unmatched (why).
CREATE TABLE playlist_imports (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (trim(name) != ''),
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'done', 'failed')),
    -- How it went, once it's done or failed.
    message TEXT,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    CHECK ((state IN ('done', 'failed')) = (finished_at IS NOT NULL))
) STRICT;
CREATE INDEX playlist_imports_by_user ON playlist_imports (user_id, created_at);

CREATE TABLE playlist_import_tracks (
    import_id INTEGER NOT NULL REFERENCES playlist_imports (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position > 0),
    title TEXT NOT NULL CHECK (trim(title) != ''),
    artist TEXT,
    album TEXT,
    isrc TEXT,
    length_ms INTEGER CHECK (length_ms IS NULL OR length_ms > 0),
    state TEXT NOT NULL CHECK (state IN ('pending', 'asked', 'already', 'unmatched')),
    why TEXT,
    request_id INTEGER REFERENCES requests (id) ON DELETE SET NULL,
    PRIMARY KEY (import_id, position),
    CHECK ((state = 'unmatched') = (why IS NOT NULL))
) STRICT;
CREATE INDEX playlist_import_tracks_by_request ON playlist_import_tracks (request_id);
