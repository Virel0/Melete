-- no-transaction
-- Series and Anime have quality profiles of their own. Both are Sonarr's kind of title, so a
-- profile still names its source (the app whose custom formats it scores), and now also the
-- Manage service it belongs to: movies for Radarr's, series or anime for Sonarr's. Names are
-- unique within a service, so Series and Anime can each have an "HD-1080p".
--
-- Each Sonarr profile goes where it's used: the anime preset, and a profile only anime series
-- use, to Anime; the rest to Series. One used on both sides is copied, so each side gets its
-- own, and whatever chose it keeps an equivalent choice: an anime series that chose it now
-- has Anime's copy, and one that names it from Sonarr finds the copy by the same name.
--
-- Changing the table's UNIQUE constraint means rebuilding it, which SQLite allows safely only
-- with foreign keys off (dropping the old table would otherwise fire the ON DELETE actions of
-- the tables that point at it). Hence no transaction around the migration, and one of its own
-- around the change.
PRAGMA foreign_keys = OFF;
BEGIN;

-- Where each Sonarr profile is used, and by which service: wanted series (the profile chosen in
-- Melete, else the one Sonarr names), where requests go, and import lists.
CREATE TEMP TABLE profile_uses AS
SELECT p.id AS profile_id, t.service
FROM wanted_series w
JOIN title_event_titles t ON t.kind = 'series' AND t.wanted_id = w.id
JOIN quality_profiles p ON p.source = 'sonarr'
    AND p.id = coalesce(w.profile_id,
        (SELECT q.id FROM quality_profiles q WHERE q.source = 'sonarr' AND q.name = w.quality_profile))
UNION
SELECT d.profile_id, d.kind FROM request_defaults d
WHERE d.kind IN ('series', 'anime') AND d.profile_id IS NOT NULL
UNION
SELECT p.id, CASE WHEN l.category = 'anime' THEN 'anime' ELSE 'series' END
FROM import_lists i
JOIN libraries l ON l.id = i.library_id
JOIN quality_profiles p ON p.source = 'sonarr' AND p.name = i.quality_profile
WHERE i.kind = 'series';

-- Each profile's service.
CREATE TEMP TABLE profile_homes AS
SELECT p.id AS profile_id,
    CASE WHEN p.source = 'radarr' THEN 'movies'
         WHEN p.name = 'Anime' AND p.origin = 'melete' THEN 'anime'
         WHEN EXISTS (SELECT 1 FROM profile_uses u WHERE u.profile_id = p.id AND u.service = 'anime')
              AND NOT EXISTS (SELECT 1 FROM profile_uses u
                              WHERE u.profile_id = p.id AND u.service = 'series')
             THEN 'anime'
         ELSE 'series' END AS service
FROM quality_profiles p;

CREATE TABLE quality_profiles_new (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL CHECK (source IN ('radarr', 'sonarr')),
    name TEXT NOT NULL,
    upgrade_allowed INTEGER NOT NULL CHECK (upgrade_allowed IN (0, 1)),
    -- Upgrades stop once a file reaches this quality's rank and cutoff_format_score.
    cutoff TEXT NOT NULL,
    min_format_score INTEGER NOT NULL,
    cutoff_format_score INTEGER NOT NULL,
    min_upgrade_format_score INTEGER NOT NULL,
    -- Radarr's profile language: an ISO 639-1 code, 'original' or 'any'. Sonarr has none.
    language TEXT,
    updated_at INTEGER NOT NULL,
    origin TEXT NOT NULL DEFAULT 'imported' CHECK (origin IN ('imported', 'melete')),
    -- The Manage service whose titles it's for: movies for Radarr's, series or anime for
    -- Sonarr's. Never left empty: a row written without one gets its source's (below).
    service TEXT CHECK (service IN ('movies', 'series', 'anime')),
    CHECK (service IS NULL OR (source = 'radarr') = (service = 'movies')),
    UNIQUE (service, name)
) STRICT;
INSERT INTO quality_profiles_new (id, source, name, upgrade_allowed, cutoff, min_format_score,
    cutoff_format_score, min_upgrade_format_score, language, updated_at, origin, service)
SELECT p.id, p.source, p.name, p.upgrade_allowed, p.cutoff, p.min_format_score,
    p.cutoff_format_score, p.min_upgrade_format_score, p.language, p.updated_at, p.origin,
    h.service
FROM quality_profiles p JOIN profile_homes h ON h.profile_id = p.id;
DROP TABLE quality_profiles;
ALTER TABLE quality_profiles_new RENAME TO quality_profiles;

-- A profile written without a service, as by a writer that doesn't know them, gets its source's
-- own: Movies for Radarr's, Series for Sonarr's. A column default can't depend on the source.
CREATE TRIGGER quality_profiles_service_default AFTER INSERT ON quality_profiles
WHEN NEW.service IS NULL
BEGIN
    UPDATE quality_profiles
    SET service = CASE NEW.source WHEN 'radarr' THEN 'movies' ELSE 'series' END
    WHERE id = NEW.id;
END;

-- The copies: a Sonarr profile used on the other side of its own, with ids above every one
-- there is, so each copy's id is its original's plus the same number.
CREATE TEMP TABLE profile_copies AS
SELECT h.profile_id AS old_id,
    CASE h.service WHEN 'series' THEN 'anime' ELSE 'series' END AS service,
    h.profile_id + (SELECT max(id) FROM quality_profiles) AS new_id
FROM profile_homes h
WHERE h.service IN ('series', 'anime')
  AND EXISTS (SELECT 1 FROM profile_uses u
              WHERE u.profile_id = h.profile_id AND u.service <> h.service);
INSERT INTO quality_profiles (id, source, name, upgrade_allowed, cutoff, min_format_score,
    cutoff_format_score, min_upgrade_format_score, language, updated_at, origin, service)
SELECT c.new_id, p.source, p.name, p.upgrade_allowed, p.cutoff, p.min_format_score,
    p.cutoff_format_score, p.min_upgrade_format_score, p.language, p.updated_at, p.origin,
    c.service
FROM profile_copies c JOIN quality_profiles p ON p.id = c.old_id;
INSERT INTO quality_profile_qualities (profile_id, quality, rank, group_name, allowed)
SELECT c.new_id, q.quality, q.rank, q.group_name, q.allowed
FROM profile_copies c JOIN quality_profile_qualities q ON q.profile_id = c.old_id
ORDER BY c.new_id, q.rowid;
INSERT INTO quality_profile_formats (profile_id, format_id, score)
SELECT c.new_id, f.format_id, f.score
FROM profile_copies c JOIN quality_profile_formats f ON f.profile_id = c.old_id;

-- What chose a copied profile on the copy's side now has the copy.
UPDATE wanted_series SET profile_id =
    (SELECT c.new_id FROM profile_copies c WHERE c.old_id = wanted_series.profile_id)
WHERE (SELECT t.service FROM title_event_titles t
       WHERE t.kind = 'series' AND t.wanted_id = wanted_series.id)
    = (SELECT c.service FROM profile_copies c WHERE c.old_id = wanted_series.profile_id);
UPDATE request_defaults SET profile_id =
    (SELECT c.new_id FROM profile_copies c WHERE c.old_id = request_defaults.profile_id)
WHERE kind = (SELECT c.service FROM profile_copies c WHERE c.old_id = request_defaults.profile_id);

DROP TABLE profile_uses;
DROP TABLE profile_homes;
DROP TABLE profile_copies;
COMMIT;
PRAGMA foreign_keys = ON;
