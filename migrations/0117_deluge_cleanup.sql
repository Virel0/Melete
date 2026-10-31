-- Removing a torrent from Deluge once what it brought is in the library, when that's turned on.
-- Only torrents Melete added itself are ever removed: one that was in Deluge already when it
-- was grabbed (added by another app, or by hand) is someone else's. Rows from before this
-- column count as not added here, so nothing grabbed earlier is removed.
ALTER TABLE downloads ADD COLUMN added_here INTEGER NOT NULL DEFAULT 0
    CHECK (added_here IN (0, 1));
-- What became of an imported torrent in Deluge: removed with its data kept on the seedbox
-- (kept_data) or with it (with_data), or gone (Deluge no longer had it, so nothing was
-- removed); NULL while it's still there, or for anything not removed. With when.
ALTER TABLE downloads ADD COLUMN deluge_removed TEXT
    CHECK (deluge_removed IN ('kept_data', 'with_data', 'gone'));
ALTER TABLE downloads ADD COLUMN deluge_removed_at INTEGER;

-- Title history gets a kind for it. SQLite can't change a CHECK in place, so the table is made
-- again with the same rows; the triggers on other tables that write to it name it, not its
-- definition, so they keep working.
CREATE TABLE title_events_was AS SELECT * FROM title_events;
DROP TABLE title_events;
CREATE TABLE title_events (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('searched', 'grabbed', 'imported', 'failed', 'upgraded',
                                       'taken_off', 'blocklisted', 'extra_found', 'renamed',
                                       'set_aside', 'removed_from_deluge')),
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
INSERT INTO title_events (id, at, kind, service, key, title, season, episode, search_id,
                          download_id, release, message)
SELECT id, at, kind, service, key, title, season, episode, search_id, download_id, release,
       message
FROM title_events_was;
DROP TABLE title_events_was;
CREATE INDEX title_events_by_service ON title_events (service, at);
CREATE INDEX title_events_by_title ON title_events (service, key, at);
CREATE INDEX title_events_by_time ON title_events (at);
CREATE TRIGGER title_events_kept_a_year AFTER INSERT ON title_events
BEGIN
    DELETE FROM title_events WHERE at < NEW.at - 365 * 24 * 3600;
END;

-- An imported torrent removed from Deluge.
CREATE TRIGGER title_events_removed_from_deluge AFTER UPDATE OF deluge_removed ON downloads
WHEN NEW.deluge_removed IN ('kept_data', 'with_data') AND OLD.deluge_removed IS NULL
BEGIN
    INSERT INTO title_events (at, kind, service, key, title, season, episode, download_id, release,
                              message)
    SELECT NEW.deluge_removed_at, 'removed_from_deluge', t.service, t.key, t.title, NEW.season,
           NEW.episode, NEW.id, NEW.title,
           CASE NEW.deluge_removed
               WHEN 'with_data' THEN 'removed from Deluge with its data'
               ELSE 'removed from Deluge; its data stays on the seedbox' END
    FROM title_event_titles t
    WHERE (t.kind = 'movie' AND t.wanted_id = NEW.wanted_movie_id)
       OR (t.kind = 'series' AND t.wanted_id = NEW.wanted_series_id)
       OR (t.kind = 'album' AND t.wanted_id = NEW.wanted_album_id)
       OR (t.kind = 'song' AND t.wanted_id = NEW.wanted_song_id);
END;
