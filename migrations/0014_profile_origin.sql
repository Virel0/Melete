-- Custom formats and quality profiles made or edited in Melete. Re-imports from Radarr
-- and Sonarr leave these alone. `source` keeps saying which kind of titles they're for:
-- radarr for movies, sonarr for series.
ALTER TABLE custom_formats ADD COLUMN origin TEXT NOT NULL DEFAULT 'imported'
    CHECK (origin IN ('imported', 'melete'));
ALTER TABLE quality_profiles ADD COLUMN origin TEXT NOT NULL DEFAULT 'imported'
    CHECK (origin IN ('imported', 'melete'));
