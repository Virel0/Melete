-- Soulseek searched again, a day on, for a wanted song it had nothing for or whose Soulseek
-- downloads all failed, or a wanted album whose Soulseek downloads all failed: how many times,
-- and when last.
CREATE TABLE soulseek_again (
    id INTEGER PRIMARY KEY,
    wanted_song_id INTEGER UNIQUE REFERENCES wanted_songs (id) ON DELETE CASCADE,
    wanted_album_id INTEGER UNIQUE REFERENCES wanted_albums (id) ON DELETE CASCADE,
    times INTEGER NOT NULL CHECK (times > 0),
    last_at INTEGER NOT NULL,
    CHECK ((wanted_song_id IS NULL) <> (wanted_album_id IS NULL))
) STRICT;
