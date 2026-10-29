-- What happened to each wanted title: searched, grabbed, imported, failed, upgraded (a file
-- replaced and put in the trash), taken off the wanted list, blocklisted, an extra found in its
-- folder, a file renamed (taking another's place). Append-only, and written by the database
-- itself where each thing is recorded, so no way of grabbing, importing or removing can miss it
-- (a rename by the scan that finds it). A title is known by its service and the id imports know it by (a
-- movie's TMDB id, a series' TVDB id, music by its artist's MusicBrainz id, or a song's artist
-- by name when it has none), so its history outlasts being taken off and put back. Kept a year.
CREATE TABLE title_events (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('searched', 'grabbed', 'imported', 'failed', 'upgraded',
                                       'taken_off', 'blocklisted', 'extra_found', 'renamed')),
    service TEXT NOT NULL CHECK (service IN ('movies', 'series', 'anime', 'music')),
    key TEXT NOT NULL,
    -- As it was called then.
    title TEXT NOT NULL,
    season INTEGER,
    episode INTEGER,
    search_id INTEGER REFERENCES searches (id) ON DELETE SET NULL,
    download_id INTEGER REFERENCES downloads (id) ON DELETE SET NULL,
    -- The release or file involved.
    release TEXT,
    message TEXT
) STRICT;
CREATE INDEX title_events_by_service ON title_events (service, at);
CREATE INDEX title_events_by_title ON title_events (service, key, at);
CREATE INDEX title_events_by_time ON title_events (at);
-- A folder's wanted title, for what happens in the folder (an extra found, a file renamed).
CREATE INDEX wanted_movies_by_folder ON wanted_movies (movie_id) WHERE movie_id IS NOT NULL;
CREATE INDEX wanted_series_by_folder ON wanted_series (series_id) WHERE series_id IS NOT NULL;

-- Each wanted title as history knows it. A series is anime when it's wanted as anime or its
-- folder's library (else the library new files go to) has the anime category.
CREATE VIEW title_event_titles AS
    SELECT 'movie' AS kind, w.id AS wanted_id, 'movies' AS service,
           CAST(w.tmdb_id AS TEXT) AS key, w.title
    FROM wanted_movies w
    UNION ALL
    SELECT 'series', w.id,
           CASE WHEN w.series_type = 'anime'
                     OR coalesce((SELECT l.category FROM series s JOIN libraries l ON l.id = s.library_id
                                  WHERE s.id = w.series_id),
                                 (SELECT l.category FROM libraries l WHERE l.id = w.library_id))
                        = 'anime'
                THEN 'anime' ELSE 'series' END,
           CAST(w.tvdb_id AS TEXT), w.title
    FROM wanted_series w
    UNION ALL
    SELECT 'artist', a.id, 'music', a.mb_artist_id, a.name FROM wanted_artists a
    UNION ALL
    SELECT 'album', al.id, 'music', a.mb_artist_id, al.title || ' by ' || a.name
    FROM wanted_albums al JOIN wanted_artists a ON a.id = al.wanted_artist_id
    UNION ALL
    SELECT 'song', so.id, 'music', coalesce(so.mb_artist_id, 'name:' || lower(so.artist)),
           so.title || ' by ' || so.artist
    FROM wanted_songs so;

CREATE TRIGGER title_events_kept_a_year AFTER INSERT ON title_events
BEGIN
    DELETE FROM title_events WHERE at < NEW.at - 365 * 24 * 3600;
END;

-- A search made for a title (by hand, for a request, or searching everything missing or below
-- its cutoff); an RSS sync's are kept with the sync instead.
CREATE TRIGGER title_events_searched AFTER INSERT ON searches WHEN NEW.rss_sync_id IS NULL
BEGIN
    INSERT INTO title_events (at, kind, service, key, title, season, episode, search_id, message)
    SELECT NEW.searched_at, 'searched', t.service, t.key, t.title, NEW.season, NEW.episode, NEW.id,
           (SELECT coalesce(sum(json_extract(i.value, '$.results')), 0)
            FROM json_each(NEW.indexers) i) || ' results'
    FROM title_event_titles t
    WHERE (t.kind = 'movie' AND t.wanted_id = NEW.wanted_movie_id)
       OR (t.kind = 'series' AND t.wanted_id = NEW.wanted_series_id)
       OR (t.kind = 'album' AND t.wanted_id = NEW.wanted_album_id)
       OR (t.kind = 'song' AND t.wanted_id = NEW.wanted_song_id);
END;

CREATE TRIGGER title_events_grabbed AFTER INSERT ON downloads
BEGIN
    INSERT INTO title_events (at, kind, service, key, title, season, episode, download_id, release,
                              message)
    SELECT NEW.added_at, 'grabbed', t.service, t.key, t.title, NEW.season, NEW.episode, NEW.id,
           NEW.title, NEW.source
    FROM title_event_titles t
    WHERE (t.kind = 'movie' AND t.wanted_id = NEW.wanted_movie_id)
       OR (t.kind = 'series' AND t.wanted_id = NEW.wanted_series_id)
       OR (t.kind = 'album' AND t.wanted_id = NEW.wanted_album_id)
       OR (t.kind = 'song' AND t.wanted_id = NEW.wanted_song_id);
END;

CREATE TRIGGER title_events_imported_or_failed AFTER UPDATE OF state ON downloads
WHEN NEW.state IN ('imported', 'failed') AND NEW.state IS NOT OLD.state
BEGIN
    INSERT INTO title_events (at, kind, service, key, title, season, episode, download_id, release,
                              message)
    SELECT NEW.updated_at, NEW.state, t.service, t.key, t.title, NEW.season, NEW.episode, NEW.id,
           NEW.title, NEW.message
    FROM title_event_titles t
    WHERE (t.kind = 'movie' AND t.wanted_id = NEW.wanted_movie_id)
       OR (t.kind = 'series' AND t.wanted_id = NEW.wanted_series_id)
       OR (t.kind = 'album' AND t.wanted_id = NEW.wanted_album_id)
       OR (t.kind = 'song' AND t.wanted_id = NEW.wanted_song_id);
END;

-- A file a confirmed upgrade replaced, moved to its library's trash.
CREATE TRIGGER title_events_upgraded AFTER INSERT ON trashed_files
BEGIN
    INSERT INTO title_events (at, kind, service, key, title, season, episode, download_id, release,
                              message)
    SELECT NEW.trashed_at, 'upgraded', t.service, t.key, t.title, d.season, d.episode, d.id,
           NEW.rel_path, 'replaced by ' || NEW.replaced_by
    FROM downloads d JOIN title_event_titles t
        ON (t.kind = 'movie' AND t.wanted_id = d.wanted_movie_id)
        OR (t.kind = 'series' AND t.wanted_id = d.wanted_series_id)
    WHERE d.id = NEW.download_id;
END;

-- A release blocklisted from a download or a search result: the title it was for, found by its
-- name (or a torrent's hash) among downloads first, then search results.
CREATE TRIGGER title_events_blocklisted AFTER INSERT ON blocklist
BEGIN
    INSERT INTO title_events (at, kind, service, key, title, season, episode, download_id, release)
    SELECT NEW.added_at, 'blocklisted', t.service, t.key, t.title, x.season, x.episode,
           x.download_id, NEW.title
    FROM (SELECT * FROM (
              SELECT 0 AS rank, d.id AS download_id, d.wanted_movie_id, d.wanted_series_id,
                     d.wanted_album_id, d.wanted_song_id, d.season, d.episode
              FROM downloads d
              WHERE lower(d.title) = lower(NEW.title) OR d.infohash = NEW.infohash
              UNION ALL
              SELECT 1, NULL, s.wanted_movie_id, s.wanted_series_id, s.wanted_album_id,
                     s.wanted_song_id, s.season, s.episode
              FROM release_decisions r JOIN searches s ON s.id = r.search_id
              WHERE lower(r.title) = lower(NEW.title))
          ORDER BY rank LIMIT 1) x
    JOIN title_event_titles t
        ON (t.kind = 'movie' AND t.wanted_id = x.wanted_movie_id)
        OR (t.kind = 'series' AND t.wanted_id = x.wanted_series_id)
        OR (t.kind = 'album' AND t.wanted_id = x.wanted_album_id)
        OR (t.kind = 'song' AND t.wanted_id = x.wanted_song_id);
END;

-- An extra (a trailer, a featurette…) newly found in a wanted title's folder.
CREATE TRIGGER title_events_extra_found AFTER INSERT ON media_extras
BEGIN
    INSERT INTO title_events (at, kind, service, key, title, release, message)
    SELECT CAST(strftime('%s', 'now') AS INTEGER), 'extra_found', t.service, t.key, t.title,
           NEW.rel_path, NEW.kind
    FROM title_event_titles t
    WHERE (t.kind = 'movie'
           AND t.wanted_id IN (SELECT id FROM wanted_movies WHERE movie_id = NEW.movie_id))
       OR (t.kind = 'series'
           AND t.wanted_id IN (SELECT id FROM wanted_series WHERE series_id = NEW.series_id));
END;

-- Taken off the wanted list: by hand, or by an import that no longer lists it (an artist's
-- albums go with it, each recorded).
CREATE TRIGGER title_events_movie_taken_off BEFORE DELETE ON wanted_movies
BEGIN
    INSERT INTO title_events (at, kind, service, key, title)
    SELECT CAST(strftime('%s', 'now') AS INTEGER), 'taken_off', t.service, t.key, t.title
    FROM title_event_titles t WHERE t.kind = 'movie' AND t.wanted_id = OLD.id;
END;
CREATE TRIGGER title_events_series_taken_off BEFORE DELETE ON wanted_series
BEGIN
    INSERT INTO title_events (at, kind, service, key, title)
    SELECT CAST(strftime('%s', 'now') AS INTEGER), 'taken_off', t.service, t.key, t.title
    FROM title_event_titles t WHERE t.kind = 'series' AND t.wanted_id = OLD.id;
END;
CREATE TRIGGER title_events_artist_taken_off BEFORE DELETE ON wanted_artists
BEGIN
    INSERT INTO title_events (at, kind, service, key, title)
    SELECT CAST(strftime('%s', 'now') AS INTEGER), 'taken_off', t.service, t.key, t.title
    FROM title_event_titles t WHERE t.kind = 'artist' AND t.wanted_id = OLD.id;
END;
CREATE TRIGGER title_events_album_taken_off BEFORE DELETE ON wanted_albums
BEGIN
    INSERT INTO title_events (at, kind, service, key, title)
    SELECT CAST(strftime('%s', 'now') AS INTEGER), 'taken_off', t.service, t.key, t.title
    FROM title_event_titles t WHERE t.kind = 'album' AND t.wanted_id = OLD.id;
END;
CREATE TRIGGER title_events_song_taken_off BEFORE DELETE ON wanted_songs
BEGIN
    INSERT INTO title_events (at, kind, service, key, title)
    SELECT CAST(strftime('%s', 'now') AS INTEGER), 'taken_off', t.service, t.key, t.title
    FROM title_event_titles t WHERE t.kind = 'song' AND t.wanted_id = OLD.id;
END;
