-- What a title folder already holds beside its videos, as scans read it (never written there):
-- the ids its .nfo files give, and its own poster and backdrop.

-- The ids, as JSON ({"tmdb": [603], "imdb": ["tt0133093"], "tvdb": []}); NULL when no .nfo
-- gives any. More than one of a kind means the files disagree.
ALTER TABLE movies ADD COLUMN nfo_ids TEXT;
-- The .nfo files as last read (path, size and date of each), so they're read again only when
-- one changes; NULL when there are none.
ALTER TABLE movies ADD COLUMN nfo_read TEXT;
-- The poster and backdrop images, by path inside the library, and the artwork name each is
-- served under (unique to the file as it is, saying nothing about its path).
ALTER TABLE movies ADD COLUMN local_poster TEXT;
ALTER TABLE movies ADD COLUMN local_poster_key TEXT;
ALTER TABLE movies ADD COLUMN local_backdrop TEXT;
ALTER TABLE movies ADD COLUMN local_backdrop_key TEXT;

ALTER TABLE series ADD COLUMN nfo_ids TEXT;
ALTER TABLE series ADD COLUMN nfo_read TEXT;
ALTER TABLE series ADD COLUMN local_poster TEXT;
ALTER TABLE series ADD COLUMN local_poster_key TEXT;
ALTER TABLE series ADD COLUMN local_backdrop TEXT;
ALTER TABLE series ADD COLUMN local_backdrop_key TEXT;

-- The artwork route finds a title's own image by its name.
CREATE INDEX movies_local_poster ON movies (local_poster_key) WHERE local_poster_key IS NOT NULL;
CREATE INDEX movies_local_backdrop ON movies (local_backdrop_key)
    WHERE local_backdrop_key IS NOT NULL;
CREATE INDEX series_local_poster ON series (local_poster_key) WHERE local_poster_key IS NOT NULL;
CREATE INDEX series_local_backdrop ON series (local_backdrop_key)
    WHERE local_backdrop_key IS NOT NULL;
