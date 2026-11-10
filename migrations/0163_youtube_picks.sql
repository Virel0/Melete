-- A YouTube video an admin chose for a wanted song by hand, waiting for the YouTube worker to
-- fetch it as the song; and whether a song's try is one chosen by hand (taken as the song
-- without matching, its audio's length not checked against the song's).
CREATE TABLE song_youtube_picks (
    wanted_song_id INTEGER PRIMARY KEY REFERENCES wanted_songs (id) ON DELETE CASCADE,
    video TEXT NOT NULL CHECK (length(video) = 11 AND video NOT GLOB '*[^A-Za-z0-9_-]*'),
    asked_at INTEGER NOT NULL
) STRICT;
ALTER TABLE song_youtube ADD COLUMN by_hand INTEGER NOT NULL DEFAULT 0 CHECK (by_hand IN (0, 1));
