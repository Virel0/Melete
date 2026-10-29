-- Colours from album covers: a background and four text colours for the apps, worked out
-- once per cover. `source` says which cover they came from (the file, tag or archive picture,
-- and its bytes' hash), so a changed cover is measured again; `error` keeps why one couldn't be.
CREATE TABLE music_cover_colours (
    album_id INTEGER PRIMARY KEY REFERENCES music_albums (id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    colours TEXT CHECK (colours IS NULL OR json_valid(colours)),
    error TEXT,
    measured_at INTEGER NOT NULL,
    CHECK ((colours IS NULL) != (error IS NULL))
) STRICT;
