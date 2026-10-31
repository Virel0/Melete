-- The width seek-bar previews were last made at for each file version, so a new width chosen in
-- Settings makes them again. Everything made before was 320 pixels wide. When making them at a
-- new width fails, the old previews stay and this still says the new width, so it isn't tried
-- again until the width or the file changes; the previews' own index says their real size.
ALTER TABLE media_trickplay
    ADD COLUMN width INTEGER NOT NULL DEFAULT 320 CHECK (width IN (320, 480, 640));
