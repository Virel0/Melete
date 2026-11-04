-- The tall (9:16) motion cover made beside the square one, for full-screen players: made for
-- every cover the renderer takes, including one whose square loop it skipped. Its name in the
-- motion covers folder under the config folder.
ALTER TABLE music_motion_covers ADD COLUMN tall_file TEXT
    CHECK (tall_file IS NULL OR state IN ('done', 'skipped'));
