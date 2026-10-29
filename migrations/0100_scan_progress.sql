-- The scan running now, if any, and how far it has got, for the live event stream: a row while
-- a library (or one of its folders) is being scanned, written at most once a second while its
-- folders are walked, and removed when the scan ends. Kept in the database rather than in
-- memory so it belongs to the one server whose scan it is. Rows a stopped server left are
-- removed at startup.
CREATE TABLE scan_progress (
    library_id INTEGER PRIMARY KEY REFERENCES libraries (id),
    -- 1 for the whole library, 0 for one title folder.
    whole INTEGER NOT NULL CHECK (whole IN (0, 1)),
    folders_done INTEGER NOT NULL DEFAULT 0 CHECK (folders_done >= 0),
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
