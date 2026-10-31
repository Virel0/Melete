-- Each file a download put in the library, as it was when placed: its size, when it was last
-- changed (in nanoseconds) and, where the file system gives them, its inode and device. A file is
-- Melete's own import only while the file at that path still matches, so a file replaced by
-- hand, or another put back at the same name, is never taken for it.
CREATE TABLE placed_files (
    download_id INTEGER NOT NULL REFERENCES downloads (id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified_ns INTEGER NOT NULL,
    inode INTEGER,
    device INTEGER,
    PRIMARY KEY (download_id, path)
) STRICT;
CREATE INDEX placed_files_by_path ON placed_files (path);
