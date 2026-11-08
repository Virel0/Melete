-- The size and date of the source file a copy was made from, written when making starts, so
-- the source's seek-bar previews go with the copy only while they are for that same version.
-- Copies made before this have neither, and get no previews.
ALTER TABLE offline_copies ADD COLUMN source_size INTEGER;
ALTER TABLE offline_copies ADD COLUMN source_modified_at INTEGER;
