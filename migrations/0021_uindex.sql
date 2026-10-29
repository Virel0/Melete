-- no-transaction
-- Uindex, reached through FlareSolverr, as an indexer kind of its own. Widening the kind
-- check rebuilds the table the way 0020 did, foreign keys off for the rebuild only.
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE indexers_new (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    -- The endpoint: a Torznab feed (http://192.168.1.10:9696/1/api), https://apibay.org, or
    -- https://nyaa.si.
    url TEXT NOT NULL,
    -- Sent as the apikey parameter of a Torznab feed; empty where none is needed. A secret:
    -- never in API responses, errors or logs.
    api_key TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    categories TEXT NOT NULL CHECK (json_valid(categories)),
    movie_search_params TEXT NOT NULL CHECK (json_valid(movie_search_params)),
    tv_search_params TEXT NOT NULL CHECK (json_valid(tv_search_params)),
    source TEXT NOT NULL CHECK (source IN ('prowlarr', 'melete')),
    -- The indexer's id in Prowlarr; none for Melete's own.
    source_id INTEGER,
    -- How it's reached.
    kind TEXT NOT NULL DEFAULT 'torznab' CHECK (kind IN ('torznab', 'apibay', 'nyaa', 'uindex')),
    added_at INTEGER NOT NULL,
    UNIQUE (source, source_id),
    CHECK (source = 'melete' OR source_id IS NOT NULL)
) STRICT;
INSERT INTO indexers_new (id, name, url, api_key, enabled, categories, movie_search_params,
    tv_search_params, source, source_id, kind, added_at)
SELECT id, name, url, api_key, enabled, categories, movie_search_params, tv_search_params,
    source, source_id, kind, added_at
FROM indexers;
DROP TABLE indexers;
ALTER TABLE indexers_new RENAME TO indexers;
COMMIT;
PRAGMA foreign_keys = ON;
