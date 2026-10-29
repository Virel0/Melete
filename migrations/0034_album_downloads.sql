-- Grabbing wanted albums: a download can be for a wanted album. Importing album
-- downloads comes later; until then a finished one waits on the seedbox.
ALTER TABLE downloads ADD COLUMN wanted_album_id INTEGER REFERENCES wanted_albums (id) ON DELETE SET NULL;
-- At most one wanted title, as the table's own check says for movies and series; a column added
-- later can't join that check, so triggers hold it.
CREATE TRIGGER downloads_one_title_insert BEFORE INSERT ON downloads
WHEN NEW.wanted_album_id IS NOT NULL
     AND (NEW.wanted_movie_id IS NOT NULL OR NEW.wanted_series_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'a download is for one wanted title');
END;
CREATE TRIGGER downloads_one_title_update BEFORE UPDATE ON downloads
WHEN NEW.wanted_album_id IS NOT NULL
     AND (NEW.wanted_movie_id IS NOT NULL OR NEW.wanted_series_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'a download is for one wanted title');
END;
