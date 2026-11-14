-- Dead torrents. seedless_since: since when Deluge has seen no seeder of a download (none
-- connected, none in the tracker's count, no full copy among its peers) and no progress; NULL
-- while one is seen, after progress, or before Melete has looked. dead_at: when it was found
-- dead (seedless and no progress for a day, nothing else to grab), removed from Deluge and its
-- release blocklisted.
ALTER TABLE downloads ADD COLUMN seedless_since INTEGER;
ALTER TABLE downloads ADD COLUMN dead_at INTEGER;
