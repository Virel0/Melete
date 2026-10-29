-- Which works are classical: ones with parts (movements), or with a track in a classical
-- genre. Only they and their composers are browsed; pop songs have composers and works too.
ALTER TABLE music_works ADD COLUMN classical INTEGER NOT NULL DEFAULT 0
    CHECK (classical IN (0, 1));
