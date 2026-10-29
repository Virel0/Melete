-- Outside services set up from the web UI, each value under the environment variable it
-- stands for. A value set in the environment wins over the one here.
CREATE TABLE settings (
    name TEXT PRIMARY KEY CHECK (name GLOB 'MELETE_[A-Z]*'),
    value TEXT NOT NULL CHECK (value != ''),
    updated_at INTEGER NOT NULL
) STRICT;
-- compose.yaml turned MusicBrainz matching on by default; that default now lives here, where
-- an admin can turn it off.
INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_MUSICBRAINZ', 'on', 0);
