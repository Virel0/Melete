-- Trailers online are gone: titles' video lists from TMDB, kept for them, are dropped with the
-- setting that turned them on. Trailers in titles' folders are unaffected.
ALTER TABLE tmdb_movies DROP COLUMN videos;
ALTER TABLE tmdb_movies DROP COLUMN videos_fetched_at;
ALTER TABLE tmdb_series DROP COLUMN videos;
ALTER TABLE tmdb_series DROP COLUMN videos_fetched_at;
DELETE FROM settings WHERE name = 'MELETE_TRAILERS_ONLINE';
