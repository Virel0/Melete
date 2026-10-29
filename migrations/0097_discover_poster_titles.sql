-- Which titles a discovery answer listed each remembered poster for (a movie's or, with tv, a
-- series' TMDB id), so a user with a highest rating is served a listed poster only for a title
-- rated within it. A poster's titles go when the poster is forgotten.
CREATE TABLE discover_poster_titles (
    file TEXT NOT NULL REFERENCES discover_posters (file) ON DELETE CASCADE,
    tv INTEGER NOT NULL CHECK (tv IN (0, 1)),
    tmdb_id INTEGER NOT NULL,
    PRIMARY KEY (file, tv, tmdb_id)
) STRICT;
