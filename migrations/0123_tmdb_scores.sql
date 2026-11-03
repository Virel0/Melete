-- TMDB's score for each title (its vote average, 0 to 10) and how many votes it rests on, kept
-- with the rest of its details for "Highly rated, not seen yet". NULL until the details are
-- fetched again; a title fetched before this is asked for once more, for its score alone.
ALTER TABLE tmdb_movies ADD COLUMN score REAL CHECK (score IS NULL OR score BETWEEN 0 AND 10);
ALTER TABLE tmdb_movies ADD COLUMN score_votes INTEGER CHECK (score_votes IS NULL OR score_votes >= 0);
ALTER TABLE tmdb_series ADD COLUMN score REAL CHECK (score IS NULL OR score BETWEEN 0 AND 10);
ALTER TABLE tmdb_series ADD COLUMN score_votes INTEGER CHECK (score_votes IS NULL OR score_votes >= 0);
