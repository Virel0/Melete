-- Playing music: stream links like video's (play_sessions), each for one
-- track as it is or converted for the device, and what each track needs for gapless playback.
--
-- A link carries its own token, tied to the token that asked for it, so signing out or
-- revoking the device ends its streams. Only the SHA-256 is stored.
CREATE TABLE music_streams (
    id INTEGER PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    token_id INTEGER NOT NULL REFERENCES tokens (id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES music_tracks (id),
    format TEXT NOT NULL CHECK (format IN ('original', 'alac', 'flac', 'aac', 'opus', 'mp3')),
    -- The converted file's name in the transcode folder's music/; NULL for the original.
    converted TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    CHECK ((format = 'original') = (converted IS NULL))
) STRICT;
CREATE INDEX music_streams_expiry ON music_streams (expires_at);

-- Samples a player drops at the start (encoder delay) and the end (padding); NULL when the
-- file doesn't say. Lossless files have neither.
ALTER TABLE music_tracks ADD COLUMN encoder_delay INTEGER;
ALTER TABLE music_tracks ADD COLUMN padding INTEGER;
-- Already-scanned tracks are read again on the next scan to find them.
UPDATE music_tracks SET size = -1;
