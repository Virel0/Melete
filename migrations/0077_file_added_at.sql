-- When each video file came into the library, for sorting titles by what was added lately. A
-- file for the same movie, or for exactly the same episodes, as one already known counts from
-- when the first of them came in, so an upgrade, another version or a renamed file isn't new.
-- Scans set it; files already known are given when they were last changed (never later than
-- now), the best there is for them.
ALTER TABLE media_files ADD COLUMN added_at INTEGER;

UPDATE media_files SET added_at = (
    SELECT min(o.modified_at) FROM media_files o WHERE o.movie_id = media_files.movie_id
)
WHERE movie_id IS NOT NULL;

UPDATE media_files SET added_at = (
    SELECT min(o.modified_at) FROM media_files o
    WHERE o.series_id = media_files.series_id AND o.season = media_files.season
      AND o.episode = media_files.episode
      AND coalesce(o.episode_end, o.episode) = coalesce(media_files.episode_end, media_files.episode)
)
WHERE season IS NOT NULL;

UPDATE media_files SET added_at = modified_at WHERE added_at IS NULL;

UPDATE media_files SET added_at = CAST(strftime('%s', 'now') AS INTEGER)
WHERE added_at > CAST(strftime('%s', 'now') AS INTEGER);
