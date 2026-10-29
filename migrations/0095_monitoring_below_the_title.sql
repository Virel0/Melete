-- Monitoring a season or an episode, and an artist's rule for new albums, set in Melete. An
-- import from Sonarr keeps a season's or episode's choice made here unless Sonarr's own value
-- for it changed since the import before (`arr_monitored` is what Sonarr said last; NULL when it
-- hasn't said); a Lidarr import keeps the rule for new albums set here.
ALTER TABLE wanted_seasons ADD COLUMN monitored_here INTEGER NOT NULL DEFAULT 0
    CHECK (monitored_here IN (0, 1));
ALTER TABLE wanted_seasons ADD COLUMN arr_monitored INTEGER CHECK (arr_monitored IN (0, 1));
ALTER TABLE wanted_episode_overrides ADD COLUMN monitored_here INTEGER NOT NULL DEFAULT 0
    CHECK (monitored_here IN (0, 1));
ALTER TABLE wanted_episode_overrides ADD COLUMN arr_monitored INTEGER
    CHECK (arr_monitored IN (0, 1));
ALTER TABLE wanted_artists ADD COLUMN monitor_new_here INTEGER NOT NULL DEFAULT 0
    CHECK (monitor_new_here IN (0, 1));

-- What a series from Sonarr has now is what Sonarr said.
UPDATE wanted_seasons SET arr_monitored = monitored
WHERE wanted_series_id IN (SELECT id FROM wanted_series WHERE source = 'sonarr');
UPDATE wanted_episode_overrides SET arr_monitored = monitored
WHERE wanted_series_id IN (SELECT id FROM wanted_series WHERE source = 'sonarr');
