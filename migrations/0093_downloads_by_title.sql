-- A title's downloads, found without reading every download: the managed lists gather each
-- title's newest download, and a title's own step, history and searches look its up by id.
CREATE INDEX downloads_by_movie ON downloads (wanted_movie_id, id) WHERE wanted_movie_id IS NOT NULL;
CREATE INDEX downloads_by_series ON downloads (wanted_series_id, id)
    WHERE wanted_series_id IS NOT NULL;
CREATE INDEX downloads_by_album ON downloads (wanted_album_id, id) WHERE wanted_album_id IS NOT NULL;
