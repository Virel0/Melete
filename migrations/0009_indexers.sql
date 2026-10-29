-- Torrent indexers, reached over Torznab. Imported from Prowlarr: each of its torrent
-- indexers has its own feed at {prowlarr}/{id}/api, which takes Prowlarr's API key.

CREATE TABLE indexers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    -- The Torznab endpoint, e.g. http://192.168.1.10:9696/1/api.
    url TEXT NOT NULL,
    -- Sent as the apikey parameter. A secret: never in API responses, errors or logs.
    api_key TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    -- Standard Torznab category ids the indexer has (2000s movies, 5000s TV), a JSON array.
    categories TEXT NOT NULL CHECK (json_valid(categories)),
    -- The parameters its movie and TV searches take ("q", "imdbid", "tvdbid", "season"...),
    -- JSON arrays.
    movie_search_params TEXT NOT NULL CHECK (json_valid(movie_search_params)),
    tv_search_params TEXT NOT NULL CHECK (json_valid(tv_search_params)),
    source TEXT NOT NULL CHECK (source IN ('prowlarr')),
    -- The indexer's id in the source.
    source_id INTEGER NOT NULL,
    added_at INTEGER NOT NULL,
    UNIQUE (source, source_id)
) STRICT;
