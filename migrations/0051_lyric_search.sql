-- Searching music by its lyrics: each kept track's lyrics line by line, with its start,
-- indexed for full-text search. Kept in step with music_lyrics by the code that writes it.
CREATE TABLE music_lyric_lines (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL REFERENCES music_tracks (id) ON DELETE CASCADE,
    -- Its place in the lyrics, from 0.
    position INTEGER NOT NULL,
    -- Seconds into the track, for synced lyrics.
    start_seconds REAL,
    text TEXT NOT NULL
) STRICT;
CREATE INDEX music_lyric_lines_by_track ON music_lyric_lines (track_id);

-- The words of each line, accents folded, for MATCH; the lines themselves stay above.
CREATE VIRTUAL TABLE music_lyric_search USING fts5 (
    text,
    content = 'music_lyric_lines',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER music_lyric_lines_added AFTER INSERT ON music_lyric_lines BEGIN
    INSERT INTO music_lyric_search (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER music_lyric_lines_removed AFTER DELETE ON music_lyric_lines BEGIN
    INSERT INTO music_lyric_search (music_lyric_search, rowid, text)
        VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER music_lyric_lines_changed AFTER UPDATE ON music_lyric_lines BEGIN
    INSERT INTO music_lyric_search (music_lyric_search, rowid, text)
        VALUES ('delete', old.id, old.text);
    INSERT INTO music_lyric_search (rowid, text) VALUES (new.id, new.text);
END;

-- What the pass reading lyrics from the library last saw of each track, so an unchanged track
-- and its .lrc aren't read again.
CREATE TABLE music_lyrics_seen (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    track_size INTEGER NOT NULL,
    track_modified_at INTEGER NOT NULL,
    -- The .lrc beside it: its modification time in nanoseconds, or none when there's none.
    sidecar_modified_at INTEGER
) STRICT;
