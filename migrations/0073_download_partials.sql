-- The hidden part files a download's copies home are written under (decision 32), each
-- recorded before it's created, so removing the download can delete them, and only them.
-- Absolute paths, a JSON array.
ALTER TABLE downloads ADD COLUMN partials TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(partials) AND json_type(partials) = 'array');
