-- Whether the Watch home's hero plays its picks' trailers for a user: on screens larger than a
-- phone's (the default), on every screen, or never.
ALTER TABLE user_playback ADD COLUMN hero_trailers TEXT NOT NULL DEFAULT 'large_screens'
    CHECK (hero_trailers IN ('off', 'large_screens', 'everywhere'));
