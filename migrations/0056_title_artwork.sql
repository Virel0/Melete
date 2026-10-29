-- Textless backdrops and logos, picked from TMDB's images by vote when a title's artwork
-- is first asked for, and asked for again after 30 days (images_fetched_at).
ALTER TABLE tmdb_movies ADD COLUMN textless_backdrop_path TEXT;
ALTER TABLE tmdb_movies ADD COLUMN logo_path TEXT;
ALTER TABLE tmdb_movies ADD COLUMN images_fetched_at INTEGER;
ALTER TABLE tmdb_series ADD COLUMN textless_backdrop_path TEXT;
ALTER TABLE tmdb_series ADD COLUMN logo_path TEXT;
ALTER TABLE tmdb_series ADD COLUMN images_fetched_at INTEGER;

-- Where a backdrop's subject is, so apps crop around it: x and y from 0 (left, top) to 1, by the
-- image's file name (the same at every size).
CREATE TABLE artwork_focus (
    file TEXT PRIMARY KEY,
    x REAL NOT NULL CHECK (x BETWEEN 0 AND 1),
    y REAL NOT NULL CHECK (y BETWEEN 0 AND 1)
) STRICT;
