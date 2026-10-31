-- no-transaction
-- A file an admin set aside as the wrong one gets its own event, "set_aside", instead of the
-- "upgraded" one every trashed file used to get. title_events is rebuilt to widen its kind
-- check (0096 isn't edited); the triggers on other tables that write to it find it by name.
-- legacy_alter_table keeps the rename from checking those triggers while the table is away.
PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN;
CREATE TABLE title_events_new (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('searched', 'grabbed', 'imported', 'failed', 'upgraded',
                                       'taken_off', 'blocklisted', 'extra_found', 'renamed',
                                       'set_aside')),
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
INSERT INTO title_events_new (id, at, kind, service, key, title, season, episode, search_id,
                              download_id, release, message)
SELECT id, at, kind, service, key, title, season, episode, search_id, download_id, release, message
FROM title_events;
DROP TABLE title_events;
ALTER TABLE title_events_new RENAME TO title_events;
CREATE INDEX title_events_by_service ON title_events (service, at);
CREATE INDEX title_events_by_title ON title_events (service, key, at);
CREATE INDEX title_events_by_time ON title_events (at);
CREATE TRIGGER title_events_kept_a_year AFTER INSERT ON title_events
BEGIN
    DELETE FROM title_events WHERE at < NEW.at - 365 * 24 * 3600;
END;

-- A file a confirmed upgrade replaced, moved to its library's trash.
DROP TRIGGER title_events_upgraded;
CREATE TRIGGER title_events_upgraded AFTER INSERT ON trashed_files WHEN NEW.reason = 'upgrade'
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

-- A file an admin set aside as the wrong one for its title, moved to its library's trash.
CREATE TRIGGER title_events_set_aside AFTER INSERT ON trashed_files WHEN NEW.reason = 'wrong'
BEGIN
    INSERT INTO title_events (at, kind, service, key, title, season, episode, download_id, release,
                              message)
    SELECT NEW.trashed_at, 'set_aside', t.service, t.key, t.title,
           coalesce(f.season, d.season), coalesce(f.episode, d.episode), d.id, NEW.rel_path,
           'set aside as the wrong file, from ' || d.title
    FROM downloads d JOIN title_event_titles t
        ON (t.kind = 'movie' AND t.wanted_id = d.wanted_movie_id)
        OR (t.kind = 'series' AND t.wanted_id = d.wanted_series_id)
    LEFT JOIN media_files f ON f.library_id = NEW.library_id AND f.rel_path = NEW.rel_path
    WHERE d.id = NEW.download_id;
END;
COMMIT;
PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
