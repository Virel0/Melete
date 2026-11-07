-- Whether the players pass over a recap, intro or preview by themselves for a user, once a play;
-- never the credits. Off until the user turns it on.
ALTER TABLE user_playback ADD COLUMN auto_skip INTEGER NOT NULL DEFAULT 0 CHECK (auto_skip IN (0, 1));
