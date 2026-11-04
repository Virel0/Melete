-- Films whose releases at a high frame rate are wanted, by TMDB id: a movie's search turns down
-- a release named as converted to a high frame rate for any film not listed here (unless
-- MELETE_ALLOW_HFR_MOVIES is on), and a film listed here isn't noted in health for playing at
-- one. Admins add and take off films; it starts with those really made at a high frame rate:
-- Billy Lynn's Long Halftime Walk, Gemini Man and the three Hobbit films. By TMDB id rather
-- than the library's movie, so it holds for a film before it's in the library.
CREATE TABLE high_frame_rate_films (
    tmdb_id INTEGER PRIMARY KEY CHECK (tmdb_id > 0),
    added_at INTEGER NOT NULL
) STRICT;
INSERT INTO high_frame_rate_films (tmdb_id, added_at) VALUES
    (293767, 0), (453405, 0), (49051, 0), (57158, 0), (122917, 0);
