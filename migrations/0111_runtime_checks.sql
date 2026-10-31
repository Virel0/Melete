-- Downloads of a wanted title held back because they didn't run as long as TMDB says, or their
-- running time couldn't be read. After three, Melete stops searching for the title by itself
-- until a download of it is imported or an admin says its runtime is right after all; that
-- override holds while TMDB's runtime for it stays what it was.
CREATE TABLE runtime_checks (
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'series')),
    -- The wanted movie's or series' row id.
    wanted_id INTEGER NOT NULL,
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    -- The latest failures, newest last: [{"at", "download_id", "release", "reason"}].
    reasons TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(reasons) AND json_type(reasons) = 'array'),
    -- How long the last held-back file's main video ran, when it could be read.
    last_seconds REAL,
    -- When an admin said the runtime is right after all, the running time to check against
    -- from then (NULL: no check), and TMDB's runtime then, which the override holds for.
    override_at INTEGER,
    override_seconds REAL,
    override_tmdb TEXT,
    PRIMARY KEY (kind, wanted_id)
) STRICT;

CREATE TRIGGER runtime_checks_movie_gone AFTER DELETE ON wanted_movies
BEGIN
    DELETE FROM runtime_checks WHERE kind = 'movie' AND wanted_id = OLD.id;
END;
CREATE TRIGGER runtime_checks_series_gone AFTER DELETE ON wanted_series
BEGIN
    DELETE FROM runtime_checks WHERE kind = 'series' AND wanted_id = OLD.id;
END;
