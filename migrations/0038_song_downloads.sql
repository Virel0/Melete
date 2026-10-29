-- Downloading wanted songs: a download can be for a wanted song, fetched
-- through Soulseek only.
ALTER TABLE downloads ADD COLUMN wanted_song_id INTEGER REFERENCES wanted_songs (id) ON DELETE SET NULL;

-- At most one wanted title per download, songs included; 0036's triggers are replaced.
DROP TRIGGER downloads_one_title_insert;
DROP TRIGGER downloads_one_title_update;
CREATE TRIGGER downloads_one_title_insert BEFORE INSERT ON downloads
WHEN (NEW.wanted_movie_id IS NOT NULL) + (NEW.wanted_series_id IS NOT NULL)
     + (NEW.wanted_album_id IS NOT NULL) + (NEW.wanted_song_id IS NOT NULL) > 1
BEGIN
    SELECT RAISE(ABORT, 'a download is for one wanted title');
END;
CREATE TRIGGER downloads_one_title_update BEFORE UPDATE ON downloads
WHEN (NEW.wanted_movie_id IS NOT NULL) + (NEW.wanted_series_id IS NOT NULL)
     + (NEW.wanted_album_id IS NOT NULL) + (NEW.wanted_song_id IS NOT NULL) > 1
BEGIN
    SELECT RAISE(ABORT, 'a download is for one wanted title');
END;
CREATE TRIGGER downloads_song_from_soulseek BEFORE INSERT ON downloads
WHEN NEW.wanted_song_id IS NOT NULL AND NEW.source != 'soulseek'
BEGIN
    SELECT RAISE(ABORT, 'a song is fetched through Soulseek');
END;

-- A song is one file of a peer's folder, so another song from the same folder is another
-- download: the file joins what makes a Soulseek download the same one.
DROP INDEX downloads_soulseek_folder;
CREATE UNIQUE INDEX downloads_soulseek_folder
    ON downloads (json_extract(soulseek, '$.username'), json_extract(soulseek, '$.folder'),
                  coalesce(CASE WHEN wanted_song_id IS NOT NULL
                                THEN json_extract(soulseek, '$.parts[0].files[0].filename') END, ''))
    WHERE source = 'soulseek' AND state != 'failed';
