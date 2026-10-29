-- Searching a whole artist: every wanted album of an artist searched for, and
-- the best result grabbed, one album at a time in the background. A run keeps what happened to
-- each album as it goes, so it can be watched.
CREATE TABLE artist_searches (
    id INTEGER PRIMARY KEY,
    wanted_artist_id INTEGER NOT NULL REFERENCES wanted_artists (id) ON DELETE CASCADE,
    -- Whether the best accepted result of each search is grabbed.
    grab INTEGER NOT NULL CHECK (grab IN (0, 1)),
    -- queued: waiting for another run to finish; running; done; failed: see message.
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'done', 'failed')),
    -- Each album as it was dealt with: [{"wanted_album_id", "title", "skipped", "search_id",
    -- "error", "grabbed": [...]}].
    albums TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(albums)),
    message TEXT,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;
CREATE INDEX artist_searches_by_artist ON artist_searches (wanted_artist_id);
