-- Motion covers are no longer made from albums' still covers: the record of the ones made and each
-- user's choice to see them go, with the setting that turned them on. An album's own motion
-- cover, from its folder, stays (`music_albums.motion_file`).
DROP TABLE music_motion_choices;
DROP TABLE music_motion_covers;
DELETE FROM settings WHERE name = 'MELETE_MOTION_COVERS';
