-- Swaps an admin started on the Make room page: a movie, or each episode file of a series, to be
-- searched for a smaller release, slowly, one at a time. The release grabbed replaces the copy
-- as an upgrade does, its old file going to the trash once the new one plays as long.
CREATE TABLE swaps (
    id INTEGER PRIMARY KEY,
    wanted_movie_id INTEGER REFERENCES wanted_movies (id) ON DELETE CASCADE,
    wanted_series_id INTEGER REFERENCES wanted_series (id) ON DELETE CASCADE,
    -- An episode's, for a series: the file being swapped and the episode it starts at.
    season INTEGER,
    episode INTEGER,
    -- The library file being replaced.
    file_id INTEGER NOT NULL REFERENCES media_files (id),
    -- waiting: to be searched. grabbed: a smaller release is on its way (download_id). done: it
    -- replaced the file. none_found: nothing small enough was found. failed: why in reason.
    state TEXT NOT NULL CHECK (state IN ('waiting', 'grabbed', 'done', 'none_found', 'failed')),
    download_id INTEGER REFERENCES downloads (id) ON DELETE SET NULL,
    reason TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    started_at INTEGER NOT NULL,
    tried_at INTEGER,
    CHECK ((wanted_movie_id IS NULL) <> (wanted_series_id IS NULL)),
    CHECK ((wanted_series_id IS NULL) = (season IS NULL)),
    CHECK ((season IS NULL) = (episode IS NULL)),
    CHECK ((state = 'grabbed') = (download_id IS NOT NULL)),
    UNIQUE (file_id)
) STRICT;
CREATE INDEX swaps_by_state ON swaps (state, id);
