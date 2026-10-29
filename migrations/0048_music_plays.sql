-- Play history: every listen, kept with how it ended, the device and the activity tag,
-- so recommendations and the Home view learn from what was really played. Filled from Aoide's
-- `play_events` in the sync log (read after the fact, as a client would) and from apps that
-- don't sync, through the music API. Kept past the sync log's play-event retention.
CREATE TABLE music_plays (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Where it came from: 'sync' (an Aoide play event) or 'api'.
    source TEXT NOT NULL CHECK (source IN ('sync', 'api')),
    -- The client's own id for the listen (a play event's entity id): sent again, it's the same
    -- listen, changed.
    event_id TEXT NOT NULL,
    -- The track as the client named it: a Melete track id, or an id the Aoide import couldn't
    -- map, as compared elsewhere (trimmed; lowercased unless a number).
    track_key TEXT NOT NULL,
    -- The Melete track, when the key is one. No reference: a listen outlives its file.
    track_id INTEGER,
    -- The device that sent it: the sync log's device id, or the session's device name.
    device TEXT NOT NULL,
    -- Milliseconds since the epoch, as the clients record them.
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    -- How far into the track it ended, in milliseconds, when the client said.
    position_ms INTEGER CHECK (position_ms >= 0),
    completed INTEGER NOT NULL CHECK (completed IN (0, 1)),
    skipped INTEGER NOT NULL CHECK (skipped IN (0, 1)),
    -- What the listener was doing, as the app tagged it ("workout", "focus"…).
    activity TEXT,
    UNIQUE (user_id, source, event_id)
) STRICT;
CREATE INDEX music_plays_recent ON music_plays (user_id, started_at);
CREATE INDEX music_plays_by_track ON music_plays (track_id) WHERE track_id IS NOT NULL;

-- How far through the sync log play events have been read into the history.
CREATE TABLE music_plays_read (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    seq INTEGER NOT NULL
) STRICT;
INSERT INTO music_plays_read (id, seq) VALUES (1, 0);
