-- Covers of music not in the library, on only with MELETE_SEARCH_COVERS: the release groups a
-- music search for a request listed, and when it last did. The cover route serves them for 30
-- days after, fetching each once from the Cover Art Archive; housekeeping forgets older ones,
-- keeps at most 2000, and deletes their cached files. `missing_at` is when the archive last said
-- it has no front cover, or failed to give it, so it isn't offered or asked for again for a day.
CREATE TABLE search_covers (
    release_group_id TEXT PRIMARY KEY,
    listed_at INTEGER NOT NULL,
    missing_at INTEGER
) STRICT;
