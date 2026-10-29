-- A song's album: the MusicBrainz release group it's on, so the album can be fetched
-- when the track can't be found alone, or after it when that's turned on; and the album wanted
-- for it, which also tells that this has been done.
ALTER TABLE wanted_songs ADD COLUMN mb_release_group_id TEXT;
ALTER TABLE wanted_songs ADD COLUMN wanted_album_id INTEGER
    REFERENCES wanted_albums (id) ON DELETE SET NULL;
