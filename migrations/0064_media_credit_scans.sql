-- Credits found in one file alone, for movies and for episodes whose credits share no
-- sound with their season's: the run of mostly black or silent seconds that reaches its end.
-- A row says a version of a file (its size and time) was looked at; the credits count only
-- while the file is still that version, and only where matching episodes found none.
CREATE TABLE media_credit_scans (
    file_id INTEGER PRIMARY KEY REFERENCES media_files (id),
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    scanned_at INTEGER NOT NULL,
    -- Why its end couldn't be read; NULL when it was.
    error TEXT,
    -- NULL when no credits were found.
    start_seconds REAL CHECK (start_seconds >= 0),
    end_seconds REAL,
    CHECK ((start_seconds IS NULL) = (end_seconds IS NULL)),
    CHECK (end_seconds > start_seconds),
    CHECK (error IS NULL OR start_seconds IS NULL)
) STRICT;
