-- Calendar: when wanted movies come out at home, from TMDB's release dates. The
-- earliest digital and physical release in any country, as YYYY-MM-DD.
ALTER TABLE tmdb_movies ADD COLUMN digital_release TEXT;
ALTER TABLE tmdb_movies ADD COLUMN physical_release TEXT;
-- When those were last asked for; NULL until then.
ALTER TABLE tmdb_movies ADD COLUMN release_dates_at INTEGER;
