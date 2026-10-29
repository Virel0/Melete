-- Who's in a title and who made it, and the franchise a film belongs to, from TMDB's data for
-- titles in the library. People are kept by TMDB's person id, never by name, so two actors with
-- one name stay two people.
CREATE TABLE tmdb_people (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    -- Their picture on TMDB's image server, fetched only when shown.
    profile_path TEXT
) STRICT;

-- A movie's or a series' top-billed cast (with the characters they play) and key crew (with
-- their jobs), in billing order.
CREATE TABLE tmdb_credits (
    movie_tmdb_id INTEGER REFERENCES tmdb_movies (tmdb_id) ON DELETE CASCADE,
    series_tmdb_id INTEGER REFERENCES tmdb_series (tmdb_id) ON DELETE CASCADE,
    person_id INTEGER NOT NULL REFERENCES tmdb_people (id),
    role TEXT NOT NULL CHECK (role IN ('cast', 'crew')),
    character TEXT,
    job TEXT,
    position INTEGER NOT NULL,
    CHECK ((movie_tmdb_id IS NULL) <> (series_tmdb_id IS NULL)),
    CHECK ((role = 'crew') = (job IS NOT NULL))
) STRICT;

CREATE INDEX tmdb_credits_movie ON tmdb_credits (movie_tmdb_id) WHERE movie_tmdb_id IS NOT NULL;
CREATE INDEX tmdb_credits_series ON tmdb_credits (series_tmdb_id)
    WHERE series_tmdb_id IS NOT NULL;
CREATE INDEX tmdb_credits_person ON tmdb_credits (person_id);

-- A franchise as TMDB groups its films ("The Matrix Collection").
CREATE TABLE tmdb_collections (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    poster_path TEXT,
    backdrop_path TEXT
) STRICT;

ALTER TABLE tmdb_movies ADD COLUMN collection_id INTEGER REFERENCES tmdb_collections (id);
CREATE INDEX tmdb_movies_collection ON tmdb_movies (collection_id)
    WHERE collection_id IS NOT NULL;

-- When the credits were last fetched; fetched again whenever the details are.
ALTER TABLE tmdb_movies ADD COLUMN credits_fetched_at INTEGER;
ALTER TABLE tmdb_series ADD COLUMN credits_fetched_at INTEGER;
