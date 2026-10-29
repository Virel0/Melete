-- What ffprobe found in each file: container, duration and streams, so apps can tell whether
-- they can play a file as it is.
--
-- Keyed to the file's size and modification time: a file that changes is probed again.
-- Failures (a corrupt file, say) are stored too, so they aren't retried on every start.
CREATE TABLE media_probes (
    file_id INTEGER PRIMARY KEY REFERENCES media_files (id),
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    probed_at INTEGER NOT NULL,
    -- JSON as returned by the API; NULL when probing failed.
    info TEXT,
    error TEXT,
    CHECK ((info IS NULL) <> (error IS NULL))
) STRICT;
