-- Lyrics translated through LibreTranslate, kept so a song is translated into each
-- language once.
CREATE TABLE music_lyrics_translations (
    track_id INTEGER NOT NULL REFERENCES music_tracks (id) ON DELETE CASCADE,
    -- The language asked for, as LibreTranslate names it ("en", "zh-Hans").
    language TEXT NOT NULL,
    -- SHA-256 of the lyrics translated: lyrics that change are translated afresh.
    lyrics_hash TEXT NOT NULL,
    -- Each line as written to its translation, as a JSON object.
    lines TEXT NOT NULL CHECK (json_valid(lines)),
    -- Unix seconds.
    translated_at INTEGER NOT NULL,
    PRIMARY KEY (track_id, language)
) STRICT;
