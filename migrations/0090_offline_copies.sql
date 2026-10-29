-- Offline copies: a converted file a device can download and keep, made in the background
-- into the transcode folder's offline/ (<id>.mp4, and <id>.<track>.vtt for each subtitle
-- asked for), never the library. Deleted 7 days after the last download (or after it was made,
-- if never downloaded), when the one who asked deletes it, or a day after failing.
CREATE TABLE offline_copies (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES media_files (id),
    -- The transcode settings asked for, as JSON, and the text subtitle tracks to include.
    settings TEXT NOT NULL,
    subtitles TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'making', 'ready', 'failed')),
    progress REAL NOT NULL DEFAULT 0,
    error TEXT,
    estimated_bytes INTEGER NOT NULL,
    bytes INTEGER,
    created_at INTEGER NOT NULL,
    ready_at INTEGER,
    last_download_at INTEGER
) STRICT;

CREATE INDEX offline_copies_by_user ON offline_copies (user_id, file_id);
