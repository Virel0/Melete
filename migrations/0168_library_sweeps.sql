-- A run for what's missing or below its cutoff may be for one library's titles alone.
ALTER TABLE missing_searches ADD COLUMN library_id INTEGER REFERENCES libraries (id) ON DELETE SET NULL;

-- A library's Scan and search: the library scanned, then its missing titles searched for, then
-- its files below their cutoff, then the subtitles its files lack fetched, each step as asked.
CREATE TABLE library_sweeps (
    id INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    -- The step under way, or the last one reached.
    step TEXT NOT NULL CHECK (step IN ('scanning', 'missing', 'upgrades', 'subtitles', 'done')),
    -- running; done; failed: see message.
    state TEXT NOT NULL CHECK (state IN ('running', 'done', 'failed')),
    -- What was asked for besides the scan.
    missing INTEGER NOT NULL CHECK (missing IN (0, 1)),
    upgrades INTEGER NOT NULL CHECK (upgrades IN (0, 1)),
    subtitles INTEGER NOT NULL CHECK (subtitles IN (0, 1)),
    -- The runs it started, for their progress and notes.
    missing_search_id INTEGER REFERENCES missing_searches (id) ON DELETE SET NULL,
    upgrade_search_id INTEGER REFERENCES missing_searches (id) ON DELETE SET NULL,
    subtitles_fetched INTEGER NOT NULL DEFAULT 0 CHECK (subtitles_fetched >= 0),
    -- How it went, in plain words, once it has ended.
    message TEXT,
    started_by INTEGER REFERENCES users (id) ON DELETE SET NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;

-- One at a time for each library.
CREATE UNIQUE INDEX library_sweeps_one_running ON library_sweeps (library_id) WHERE state = 'running';
