-- Posters of titles not in the library, an exception to decision 70 an admin turns on:
-- those a discovery answer listed, by TMDB file name, and when it last did. The artwork route
-- serves them (w342 only) for 30 days after, and housekeeping forgets older ones, keeps at most
-- 2000, and deletes their cached files.
CREATE TABLE discover_posters (
    file TEXT PRIMARY KEY,
    listed_at INTEGER NOT NULL
) STRICT;
