-- Searching for every missing title of an area at once: wanted movies, the seasons of
-- wanted series, or wanted albums that are monitored and neither on disk nor on their way. A
-- run goes one search at a time in the background and records how far it has got, so it can be
-- watched and stopped.
CREATE TABLE missing_searches (
    id INTEGER PRIMARY KEY,
    area TEXT NOT NULL CHECK (area IN ('movies', 'series', 'albums')),
    -- running; stopping: asked to stop, finishing the search under way; done; stopped;
    -- failed: see message.
    state TEXT NOT NULL CHECK (state IN ('running', 'stopping', 'done', 'stopped', 'failed')),
    -- Searches the run set out to make (a series counts each season), and how many it has
    -- dealt with, made, and grabbed a release from.
    total INTEGER NOT NULL DEFAULT 0 CHECK (total >= 0),
    done INTEGER NOT NULL DEFAULT 0 CHECK (done >= 0),
    searched INTEGER NOT NULL DEFAULT 0 CHECK (searched >= 0),
    grabbed INTEGER NOT NULL DEFAULT 0 CHECK (grabbed >= 0),
    -- What it's searching for now, while running.
    current TEXT,
    -- Why some weren't searched for or grabbed, in plain words: ["Title: why", ...].
    notes TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(notes)),
    message TEXT,
    started_by INTEGER REFERENCES users (id) ON DELETE SET NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;
-- One run at a time per area.
CREATE UNIQUE INDEX missing_searches_one_per_area ON missing_searches (area)
    WHERE state IN ('running', 'stopping');
