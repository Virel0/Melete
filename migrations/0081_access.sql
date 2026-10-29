-- Which libraries each user may see and the highest content rating they may see, and each
-- title's content ratings from TMDB on one age scale. A user without a row sees everything, as
-- every user did before; admins always do.

CREATE TABLE user_access (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- 1: every library, those added later too. 0: only those in user_libraries.
    all_libraries INTEGER NOT NULL CHECK (all_libraries IN (0, 1)),
    -- The oldest age a title may be rated for; NULL for no limit. With one, a title without a
    -- rating Melete can read is hidden.
    max_age INTEGER CHECK (max_age BETWEEN 0 AND 18),
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE user_libraries (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, library_id)
) STRICT;

-- A movie's or series' certifications by country, as TMDB gives them, and the age they come to.
CREATE TABLE tmdb_ratings (
    tv INTEGER NOT NULL CHECK (tv IN (0, 1)),
    tmdb_id INTEGER NOT NULL,
    -- JSON object: country code to certification ({"US": "PG-13", "DE": "12"}).
    certifications TEXT NOT NULL CHECK (json_valid(certifications)
        AND json_type(certifications) = 'object'),
    -- The highest age any of them comes to; NULL when none could be read.
    age INTEGER CHECK (age BETWEEN 0 AND 18),
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (tv, tmdb_id)
) STRICT;
