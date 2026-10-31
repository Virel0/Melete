-- A title's own language, as TMDB gives it (`original_language`: ja, en, ko…), for telling
-- how alike two titles are. `language` is the language the details were fetched in, not the
-- title's. NULL until the title's details are next fetched.
ALTER TABLE tmdb_movies ADD COLUMN original_language TEXT;
ALTER TABLE tmdb_series ADD COLUMN original_language TEXT;
