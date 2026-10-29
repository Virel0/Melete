-- Each user's favourite movies and series, keyed by Melete's own title ids (never a provider id
-- or a file), so a favourite outlives its files being upgraded, renamed or missing. Scans never
-- delete title rows, so a favourite stays until its user removes it.

CREATE TABLE favourites (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    movie_id INTEGER REFERENCES movies (id),
    series_id INTEGER REFERENCES series (id),
    added_at INTEGER NOT NULL,
    CHECK ((movie_id IS NULL) <> (series_id IS NULL)),
    UNIQUE (user_id, movie_id),
    UNIQUE (user_id, series_id)
) STRICT;

CREATE INDEX favourites_newest ON favourites (user_id, added_at DESC, id DESC);

-- Raised by each change to a user's favourites, so the event stream can say when they changed.
CREATE TABLE favourite_versions (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    version INTEGER NOT NULL CHECK (version > 0)
) STRICT;
