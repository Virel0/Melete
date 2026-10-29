-- Upgrades (decision 37): a grab records the library files it would replace. Once
-- the upgrade is confirmed in place, each is moved to its library's trash folder
-- (.melete-trash, which scans skip) and deleted from there 30 days later.

-- [{"file_id", "library_id", "rel_path", "size", "modified_at", "season", "episode",
--   "episode_end"}], as the files were when grabbed.
ALTER TABLE downloads ADD COLUMN replaces TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(replaces));
-- When the replaced files were dealt with: trashed, or kept with the reason in message.
ALTER TABLE downloads ADD COLUMN retired_at INTEGER;

CREATE TABLE trashed_files (
    id INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    -- Where it was and where it is now, both relative to the library's folder.
    rel_path TEXT NOT NULL,
    trash_path TEXT NOT NULL,
    size INTEGER NOT NULL,
    download_id INTEGER REFERENCES downloads (id) ON DELETE SET NULL,
    -- The file that replaced it, relative to the library's folder.
    replaced_by TEXT NOT NULL,
    trashed_at INTEGER NOT NULL,
    deleted_at INTEGER,
    restored_at INTEGER,
    CHECK (deleted_at IS NULL OR restored_at IS NULL)
) STRICT;
