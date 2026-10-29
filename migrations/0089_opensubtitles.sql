-- Subtitles from OpenSubtitles, asked for one file version and language at a time.
--
-- What a search found, remembered so the same search isn't sent again for a week, an empty
-- one included.
CREATE TABLE opensubtitles_searches (
    file_id INTEGER NOT NULL REFERENCES media_files (id),
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    language TEXT NOT NULL,
    searched_at INTEGER NOT NULL,
    -- JSON array of what was found, as the API answers it.
    results TEXT NOT NULL,
    PRIMARY KEY (file_id, language)
) STRICT;

-- The subtitle fetched for a file version and language: one at most, kept in the data
-- folder's subtitles/fetched/<file id>-<size>-<modified>/<language>.srt.
CREATE TABLE opensubtitles_fetched (
    file_id INTEGER NOT NULL REFERENCES media_files (id),
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    language TEXT NOT NULL,
    subtitle_file_id INTEGER NOT NULL,
    release TEXT,
    hearing_impaired INTEGER NOT NULL,
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (file_id, language)
) STRICT;

-- When OpenSubtitles' downloads for the day were used up: none are asked for until then.
CREATE TABLE opensubtitles_quota (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    reset_at INTEGER NOT NULL
) STRICT;
