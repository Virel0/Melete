-- Releases an admin never wants grabbed again, from a search result or a download.
-- A release is known by its name, as indexers and Soulseek peers give it (case aside), and a
-- torrent also by its info hash once Melete has seen it, so the same torrent under another
-- name is caught at grab time.
CREATE TABLE blocklist (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL CHECK (trim(title) != ''),
    infohash TEXT,
    source TEXT NOT NULL CHECK (source IN ('torrent', 'soulseek')),
    added_at INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX blocklist_by_title ON blocklist (lower(title));
CREATE INDEX blocklist_by_infohash ON blocklist (infohash) WHERE infohash IS NOT NULL;
