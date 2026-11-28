-- A song request's downloads, found by the song: swapping arrived songs into playlists looks
-- up each pending request's imported downloads, which without this meant reading every
-- download.
CREATE INDEX downloads_by_song ON downloads (wanted_song_id, id) WHERE wanted_song_id IS NOT NULL;
