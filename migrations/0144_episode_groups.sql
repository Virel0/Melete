-- TMDB's episode groups: other orders of a series' episodes (the seasons other sites use, DVD
-- order, story arcs). A series follows TMDB's own order unless it follows one of them, and
-- everything that numbers episodes asks `episodes_in_order` rather than `tmdb_episodes`.

-- When the series' episode groups were last asked for; asked again once its metadata is newer.
ALTER TABLE tmdb_series ADD COLUMN episode_groups_fetched_at INTEGER;

CREATE TABLE tmdb_episode_groups (
    -- TMDB's id for the group.
    id TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 64),
    series_tmdb_id INTEGER NOT NULL REFERENCES tmdb_series (tmdb_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    -- TMDB's type: 1 original air date, 2 absolute, 3 DVD, 4 digital, 5 story arc,
    -- 6 production, 7 TV.
    kind INTEGER,
    -- When its episodes were fetched; NULL while it's only listed.
    fetched_at INTEGER
) STRICT;

CREATE INDEX tmdb_episode_groups_by_series ON tmdb_episode_groups (series_tmdb_id);

-- A group's episodes as seasons: its parts in TMDB's order are seasons 1, 2, ... (a part
-- called Specials is season 0), and each part's episodes in order are episodes 1, 2, ...
-- Episodes are TMDB's own (`tmdb_episodes.tmdb_id`), so they keep their titles and stills.
CREATE TABLE tmdb_episode_group_episodes (
    group_id TEXT NOT NULL REFERENCES tmdb_episode_groups (id) ON DELETE CASCADE,
    season INTEGER NOT NULL CHECK (season >= 0),
    episode INTEGER NOT NULL CHECK (episode > 0),
    episode_tmdb_id INTEGER NOT NULL,
    PRIMARY KEY (group_id, season, episode),
    UNIQUE (group_id, episode_tmdb_id)
) STRICT;

-- The order a series follows, when one was chosen: a group, or TMDB's own (`group_id` NULL,
-- which only an admin chooses). Without a row it follows TMDB's own and Melete may choose a
-- group that fits its files exactly; an admin's choice is never changed automatically.
CREATE TABLE series_orders (
    series_tmdb_id INTEGER PRIMARY KEY REFERENCES tmdb_series (tmdb_id) ON DELETE CASCADE,
    group_id TEXT REFERENCES tmdb_episode_groups (id),
    chosen_by TEXT NOT NULL CHECK (chosen_by IN ('auto', 'admin')),
    chosen_at INTEGER NOT NULL,
    CHECK (chosen_by = 'admin' OR group_id IS NOT NULL)
) STRICT;

-- Each series' episodes in the order it follows: TMDB's own, or its group's seasons and
-- episodes. A group's seasons take the place of TMDB's; specials the group leaves out stay
-- season 0 as TMDB numbers them, unless the group has specials of its own. `tmdb_season` and
-- `tmdb_episode` are TMDB's own numbers. One SELECT, not a UNION, so a query joining it reads
-- `tmdb_episodes` by its index rather than making the whole view first.
CREATE VIEW episodes_in_order AS
    SELECT e.series_tmdb_id, coalesce(g.season, e.season) AS season,
           coalesce(g.episode, e.episode) AS episode, e.tmdb_id, e.name, e.overview, e.air_date,
           e.runtime_minutes, e.still_path, e.english_fields,
           e.season AS tmdb_season, e.episode AS tmdb_episode
    FROM tmdb_episodes e
    LEFT JOIN series_orders o ON o.series_tmdb_id = e.series_tmdb_id AND o.group_id IS NOT NULL
    LEFT JOIN tmdb_episode_group_episodes g
        ON g.group_id = o.group_id AND g.episode_tmdb_id = e.tmdb_id
    WHERE o.series_tmdb_id IS NULL
       OR g.group_id IS NOT NULL
       OR (e.season = 0
           AND NOT EXISTS (SELECT 1 FROM tmdb_episode_group_episodes x
                           WHERE x.group_id = o.group_id AND x.season = 0));

-- Title history gets a kind for a series changing order. SQLite can't change a CHECK in place,
-- so the table is made again with the same rows, as migration 0117 did; the triggers on other
-- tables that write to it name it, not its definition, so they keep working.
CREATE TABLE title_events_was AS SELECT * FROM title_events;
DROP TABLE title_events;
CREATE TABLE title_events (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('searched', 'grabbed', 'imported', 'failed', 'upgraded',
                                       'taken_off', 'blocklisted', 'extra_found', 'renamed',
                                       'set_aside', 'removed_from_deluge', 'reordered')),
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
