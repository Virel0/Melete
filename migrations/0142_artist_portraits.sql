-- Artists' portraits from Wikimedia Commons, on only with MELETE_ARTIST_PORTRAITS: the artists
-- a music search for a request listed, when it last did, and what looking each up found (when,
-- the Commons file, its picture's address at one width, and its author and licence), or that
-- there's none (`file` NULL). The portrait route serves them for 30 days after they were last
-- listed, fetching each picture once; housekeeping forgets older ones, keeps at most 2000, and
-- deletes their cached files. `failed_at` is when Commons last failed to give the picture, so
-- it isn't offered or asked for again for a day.
CREATE TABLE artist_portraits (
    mb_artist_id TEXT PRIMARY KEY,
    listed_at INTEGER NOT NULL,
    looked_up_at INTEGER NOT NULL,
    file TEXT,
    picture_url TEXT,
    author TEXT,
    licence TEXT,
    licence_url TEXT,
    source_url TEXT,
    failed_at INTEGER,
    CHECK ((file IS NULL) = (picture_url IS NULL))
) STRICT;
