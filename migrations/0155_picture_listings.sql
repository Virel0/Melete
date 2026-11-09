-- Who a search or a discovery answer listed each picture of something not in the library for
-- (a discovery poster by its TMDB file name, a release group's cover, an artist's portrait), and
-- when it last did: the picture routes serve it only to those it was listed for in the last 30
-- days, so nobody learns from them what someone else searched for. A user's rows go with them;
-- a picture's go when it's forgotten, and housekeeping forgets rows past 30 days.
CREATE TABLE picture_listings (
    kind TEXT NOT NULL CHECK (kind IN ('poster', 'cover', 'portrait')),
    key TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    listed_at INTEGER NOT NULL,
    PRIMARY KEY (kind, key, user_id)
) STRICT;
