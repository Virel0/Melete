-- Collections: each listener's saved song filters, a name for several genres, a span of years
-- and "only songs I like", as the songs list takes them. Each user's are their own; they hold
-- no music, only the filter.
CREATE TABLE music_collections (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 100),
    -- JSON array of genre names, any of which fits; empty for every genre.
    genres TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(genres) AND json_type(genres) = 'array'),
    year_from INTEGER CHECK (year_from BETWEEN 1 AND 9999),
    year_to INTEGER CHECK (year_to BETWEEN 1 AND 9999),
    liked INTEGER NOT NULL DEFAULT 0 CHECK (liked IN (0, 1)),
    -- Unix seconds.
    created_at INTEGER NOT NULL,
    CHECK (year_from IS NULL OR year_to IS NULL OR year_from <= year_to)
) STRICT;

CREATE UNIQUE INDEX music_collections_name ON music_collections (user_id, name COLLATE NOCASE);
