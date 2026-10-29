-- Folded keys for searching and sorting titles: accents, case and punctuation aside, and a
-- leading article left out of the sort key. SQLite can't fold accents itself, so Melete works
-- the keys out and stores them; NULL means not yet worked out, and the lists fall back to the
-- title as it is until then. A changed title clears its keys at once, so a stale key is never
-- searched.

ALTER TABLE movies ADD COLUMN search_key TEXT;
ALTER TABLE movies ADD COLUMN sort_key TEXT;
ALTER TABLE series ADD COLUMN search_key TEXT;
ALTER TABLE series ADD COLUMN sort_key TEXT;
ALTER TABLE tmdb_movies ADD COLUMN search_key TEXT;
ALTER TABLE tmdb_movies ADD COLUMN sort_key TEXT;
ALTER TABLE tmdb_series ADD COLUMN search_key TEXT;
ALTER TABLE tmdb_series ADD COLUMN sort_key TEXT;
ALTER TABLE music_artists ADD COLUMN search_key TEXT;
ALTER TABLE music_artists ADD COLUMN sort_key TEXT;
ALTER TABLE music_albums ADD COLUMN search_key TEXT;
ALTER TABLE music_albums ADD COLUMN sort_key TEXT;
ALTER TABLE music_albums ADD COLUMN artist_sort_key TEXT;
ALTER TABLE music_tracks ADD COLUMN search_key TEXT;
ALTER TABLE music_tracks ADD COLUMN sort_key TEXT;

-- Finding the rows still to work out, without reading every row.
CREATE INDEX movies_unkeyed ON movies (id) WHERE search_key IS NULL;
CREATE INDEX series_unkeyed ON series (id) WHERE search_key IS NULL;
CREATE INDEX tmdb_movies_unkeyed ON tmdb_movies (tmdb_id) WHERE search_key IS NULL;
CREATE INDEX tmdb_series_unkeyed ON tmdb_series (tmdb_id) WHERE search_key IS NULL;
CREATE INDEX music_artists_unkeyed ON music_artists (id) WHERE search_key IS NULL;
CREATE INDEX music_albums_unkeyed ON music_albums (id) WHERE search_key IS NULL;
CREATE INDEX music_tracks_unkeyed ON music_tracks (id) WHERE search_key IS NULL;

CREATE TRIGGER movies_title_changed AFTER UPDATE OF parsed_title ON movies
WHEN OLD.parsed_title IS NOT NEW.parsed_title
BEGIN
    UPDATE movies SET search_key = NULL, sort_key = NULL WHERE id = NEW.id;
END;

CREATE TRIGGER series_title_changed AFTER UPDATE OF parsed_title ON series
WHEN OLD.parsed_title IS NOT NEW.parsed_title
BEGIN
    UPDATE series SET search_key = NULL, sort_key = NULL WHERE id = NEW.id;
END;

CREATE TRIGGER tmdb_movies_title_changed AFTER UPDATE OF title, original_title ON tmdb_movies
WHEN OLD.title IS NOT NEW.title OR OLD.original_title IS NOT NEW.original_title
BEGIN
    UPDATE tmdb_movies SET search_key = NULL, sort_key = NULL WHERE tmdb_id = NEW.tmdb_id;
END;

CREATE TRIGGER tmdb_series_title_changed AFTER UPDATE OF name, original_name ON tmdb_series
WHEN OLD.name IS NOT NEW.name OR OLD.original_name IS NOT NEW.original_name
BEGIN
    UPDATE tmdb_series SET search_key = NULL, sort_key = NULL WHERE tmdb_id = NEW.tmdb_id;
END;

CREATE TRIGGER music_artists_name_changed AFTER UPDATE OF name ON music_artists
WHEN OLD.name IS NOT NEW.name
BEGIN
    UPDATE music_artists SET search_key = NULL, sort_key = NULL WHERE id = NEW.id;
END;

CREATE TRIGGER music_albums_title_changed AFTER UPDATE OF title, artist_name ON music_albums
WHEN OLD.title IS NOT NEW.title OR OLD.artist_name IS NOT NEW.artist_name
BEGIN
    UPDATE music_albums SET search_key = NULL, sort_key = NULL, artist_sort_key = NULL
    WHERE id = NEW.id;
END;

CREATE TRIGGER music_tracks_title_changed AFTER UPDATE OF title, artist ON music_tracks
WHEN OLD.title IS NOT NEW.title OR OLD.artist IS NOT NEW.artist
BEGIN
    UPDATE music_tracks SET search_key = NULL, sort_key = NULL WHERE id = NEW.id;
END;
