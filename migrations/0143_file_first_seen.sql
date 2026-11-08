-- When a scan first found each video file, so the TMDB refresh can tell an episode file that
-- came in after its series was fetched, and fetch the series again while that episode has no
-- title yet. A file found again after going missing keeps its time. Files known before have
-- none.
ALTER TABLE media_files ADD COLUMN first_seen_at INTEGER;
