-- A film's earliest showing on TV (TMDB's release type 6), which counts like a digital release
-- for WEB and TV copies. Release dates fetched before don't have it, so they're fetched again.
ALTER TABLE tmdb_movies ADD COLUMN tv_release TEXT;
UPDATE tmdb_movies SET release_dates_at = NULL WHERE release_dates_at IS NOT NULL;
