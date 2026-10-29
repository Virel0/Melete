-- Listens a user deleted from their play history. The history is read from the sync log
-- after the fact, and a device may push a play event again with the same id (its end, say);
-- one named here stays deleted. Kept for listens sent through the API too, for the same reason.
CREATE TABLE music_plays_forgotten (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    source TEXT NOT NULL CHECK (source IN ('sync', 'api')),
    event_id TEXT NOT NULL,
    PRIMARY KEY (user_id, source, event_id)
) STRICT;
