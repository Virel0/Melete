-- Anime numbered from the first episode: "Frieren - 05" names no season.

-- The number the file's name gives when it names no season and episode.
ALTER TABLE media_files ADD COLUMN absolute_episode INTEGER CHECK (absolute_episode > 0);
-- Whether season and episode were worked out from absolute_episode through TMDB's episode
-- list (and so are kept across scans while the name gives the same number).
ALTER TABLE media_files ADD COLUMN mapped INTEGER NOT NULL DEFAULT 0 CHECK (mapped IN (0, 1));
