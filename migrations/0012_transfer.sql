-- Bringing finished downloads home.

-- Failed transfer attempts in a row, and when the next may start (they back off).
ALTER TABLE downloads ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0);
ALTER TABLE downloads ADD COLUMN next_attempt_at INTEGER;
-- The library folder it was copied into.
ALTER TABLE downloads ADD COLUMN destination TEXT;
-- Files this download has put in the library, so a restart doesn't take them for someone
-- else's.
ALTER TABLE downloads ADD COLUMN placed TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(placed));
