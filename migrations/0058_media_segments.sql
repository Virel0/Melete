-- Intros and credits found in episodes, by audio shared with other episodes of the season.
-- A scan row says a version of a file (its size and time) was looked at, found something or not;
-- segments count only while the file is still that version.
CREATE TABLE media_segment_scans (
    file_id INTEGER PRIMARY KEY REFERENCES media_files (id),
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    scanned_at INTEGER NOT NULL,
    -- Why its audio couldn't be read; NULL when it was.
    error TEXT
) STRICT;

CREATE TABLE media_segments (
    file_id INTEGER NOT NULL REFERENCES media_files (id),
    kind TEXT NOT NULL CHECK (kind IN ('intro', 'credits')),
    start_seconds REAL NOT NULL CHECK (start_seconds >= 0),
    end_seconds REAL NOT NULL,
    PRIMARY KEY (file_id, kind),
    CHECK (end_seconds > start_seconds)
) STRICT;
