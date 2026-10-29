-- Trailers online: each library title's YouTube trailers and teasers from TMDB's video list,
-- fetched only while the setting is on, after its details and again whenever they are. Kept
-- as a JSON array of {key, name, language, type, official, size, published_at}; NULL until
-- fetched.
ALTER TABLE tmdb_movies ADD COLUMN videos TEXT CHECK (videos IS NULL OR json_valid(videos));
ALTER TABLE tmdb_movies ADD COLUMN videos_fetched_at INTEGER;
ALTER TABLE tmdb_series ADD COLUMN videos TEXT CHECK (videos IS NULL OR json_valid(videos));
ALTER TABLE tmdb_series ADD COLUMN videos_fetched_at INTEGER;
