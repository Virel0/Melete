-- Compressions an admin started on the Make room page: each H.264 (or older) video file of a
-- title, converted to HEVC on the GPU one at a time while nothing else is converted. The new file
-- takes the old one's place (and its row, so progress stays) once it plays as long with the same
-- tracks and is smaller; the old one goes to the trash.
CREATE TABLE compressions (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL UNIQUE REFERENCES media_files (id),
    -- waiting, running, done (saved bytes freed), failed (why in reason).
    state TEXT NOT NULL CHECK (state IN ('waiting', 'running', 'done', 'failed')),
    reason TEXT,
    saved INTEGER CHECK (saved IS NULL OR saved >= 0),
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    CHECK ((state = 'failed') = (reason IS NOT NULL)),
    CHECK ((state = 'done') = (saved IS NOT NULL))
) STRICT;
CREATE INDEX compressions_by_state ON compressions (state, id);
