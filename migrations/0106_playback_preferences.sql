-- Each user's playback preferences, kept on the server so every app and the web player behave
-- the same for them. A user without a row has the defaults: the next episode starts by itself
-- after a 10-second countdown.

CREATE TABLE user_playback (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    autoplay_next INTEGER NOT NULL DEFAULT 1 CHECK (autoplay_next IN (0, 1)),
    countdown_seconds INTEGER NOT NULL DEFAULT 10 CHECK (countdown_seconds BETWEEN 5 AND 30),
    updated_at INTEGER NOT NULL
) STRICT;
