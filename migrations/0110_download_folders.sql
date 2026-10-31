-- The folders a download's copy home made in the library, recorded before each is made, so a
-- download held back takes away the empty ones it made, across retries and restarts, and never
-- one that was there before.
ALTER TABLE downloads ADD COLUMN created_dirs TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(created_dirs) AND json_type(created_dirs) = 'array');
