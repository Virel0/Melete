-- no-transaction
-- A third reason a file goes to the trash: removed by an admin on the Make room page, with the
-- rest of its title. trashed_files is rebuilt to widen its reason check (0114 isn't edited),
-- keeping every row and id, and the triggers on it are made again as 0116 left them.
-- legacy_alter_table keeps the rename from checking other objects while the table is away.
PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN;
CREATE TABLE trashed_files_new (
    id INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id),
    -- Where it was and where it is now, both relative to the library's folder.
    rel_path TEXT NOT NULL,
    trash_path TEXT NOT NULL,
    size INTEGER NOT NULL,
    download_id INTEGER REFERENCES downloads (id) ON DELETE SET NULL,
    -- The file that replaced it, relative to the library's folder; empty unless an upgrade.
    replaced_by TEXT NOT NULL,
    trashed_at INTEGER NOT NULL,
    deleted_at INTEGER,
    restored_at INTEGER,
    reason TEXT NOT NULL DEFAULT 'upgrade' CHECK (reason IN ('upgrade', 'wrong', 'removed')),
    CHECK (deleted_at IS NULL OR restored_at IS NULL)
) STRICT;
INSERT INTO trashed_files_new (id, library_id, rel_path, trash_path, size, download_id,
                               replaced_by, trashed_at, deleted_at, restored_at, reason)
SELECT id, library_id, rel_path, trash_path, size, download_id, replaced_by, trashed_at,
       deleted_at, restored_at, reason
FROM trashed_files;
DROP TABLE trashed_files;
ALTER TABLE trashed_files_new RENAME TO trashed_files;

-- A file a confirmed upgrade replaced, moved to its library's trash.
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
