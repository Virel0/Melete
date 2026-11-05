-- The companies behind each title, from the details TMDB already gives (a movie's production
-- companies; a series' production companies and networks), as a JSON array of names, for
-- telling a major studio's title from a small one's on the Make room page. NULL until the
-- details are fetched again; a title fetched before this is asked for once more, for them alone.
ALTER TABLE tmdb_movies ADD COLUMN companies TEXT CHECK (companies IS NULL OR json_valid(companies));
ALTER TABLE tmdb_series ADD COLUMN companies TEXT CHECK (companies IS NULL OR json_valid(companies));
