-- no-transaction
-- Indexers Melete reaches itself: The Pirate Bay through its JSON API (apibay), Nyaa
-- through its RSS feed, and Torznab feeds without Prowlarr (AnimeTosho). They're Melete's own
-- (source 'melete', no source id) and say how they're reached (kind).
--
-- Widening the source check means rebuilding the table, which SQLite allows safely only with
-- foreign keys off (its 12-step procedure): with them on, dropping the old table would fire the
-- ON DELETE actions of release_decisions (SET NULL) and rss_seen (CASCADE). Hence no
-- transaction around the migration, and one of its own around the rebuild.
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
    kind TEXT NOT NULL DEFAULT 'torznab' CHECK (kind IN ('torznab', 'apibay', 'nyaa')),
    added_at INTEGER NOT NULL,
    UNIQUE (source, source_id),
    CHECK (source = 'melete' OR source_id IS NOT NULL)
) STRICT;
INSERT INTO indexers_new (id, name, url, api_key, enabled, categories, movie_search_params,
    tv_search_params, source, source_id, added_at)
SELECT id, name, url, api_key, enabled, categories, movie_search_params, tv_search_params,
    source, source_id, added_at
FROM indexers;
DROP TABLE indexers;
ALTER TABLE indexers_new RENAME TO indexers;
COMMIT;
PRAGMA foreign_keys = ON;
