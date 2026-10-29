-- Metadata in a chosen language, with English where TMDB has no translation. Each cached title
-- keeps the language it was fetched in, so changing the setting fetches it again, and which of
-- its fields came from English instead (a JSON array: "title", "overview"; "name" for an
-- episode). Everything cached so far was fetched in English.
ALTER TABLE tmdb_movies ADD COLUMN language TEXT;
ALTER TABLE tmdb_movies ADD COLUMN english_fields TEXT;
ALTER TABLE tmdb_series ADD COLUMN language TEXT;
ALTER TABLE tmdb_series ADD COLUMN english_fields TEXT;
ALTER TABLE tmdb_episodes ADD COLUMN english_fields TEXT;

UPDATE tmdb_movies SET language = 'en-US';
UPDATE tmdb_series SET language = 'en-US';

-- A title fetched in another language sorts by that language's articles ("Le", "Der"...).
CREATE TRIGGER tmdb_movies_language_changed AFTER UPDATE OF language ON tmdb_movies
WHEN OLD.language IS NOT NEW.language
BEGIN
    UPDATE tmdb_movies SET search_key = NULL, sort_key = NULL WHERE tmdb_id = NEW.tmdb_id;
END;

CREATE TRIGGER tmdb_series_language_changed AFTER UPDATE OF language ON tmdb_series
WHEN OLD.language IS NOT NEW.language
BEGIN
    UPDATE tmdb_series SET search_key = NULL, sort_key = NULL WHERE tmdb_id = NEW.tmdb_id;
END;
