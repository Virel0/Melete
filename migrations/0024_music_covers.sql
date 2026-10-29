-- Album covers: an image in the album's folder (cover.jpg, folder.png…), a
-- looping video there as a motion cover (cover.mp4, animated.webm…), and whether a track has a
-- picture in its tags. Paths are relative to the library; scans keep them current.
ALTER TABLE music_albums ADD COLUMN cover_file TEXT;
ALTER TABLE music_albums ADD COLUMN motion_file TEXT;
ALTER TABLE music_tracks ADD COLUMN has_picture INTEGER NOT NULL DEFAULT 0
    CHECK (has_picture IN (0, 1));
-- Tracks already scanned were read without looking for pictures: an impossible size makes the
-- next scan read each of them again.
UPDATE music_tracks SET size = -1;
