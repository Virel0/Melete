-- When TMDB last said it no longer has a matched movie or series. The cached copy is kept and
-- served; TMDB is asked again a day later, not at every run, and the title is listed to be
-- matched again until TMDB has it once more or it's matched to another entry.
ALTER TABLE tmdb_movies ADD COLUMN gone_at INTEGER;
ALTER TABLE tmdb_series ADD COLUMN gone_at INTEGER;
