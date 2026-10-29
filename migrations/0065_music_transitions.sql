-- Auto DJ's transitions as they were played, beside the play history, so the rules
-- that plan them can be tuned from how they went. Sent by the app that played them.
CREATE TABLE music_transitions (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The app's own id for it: sent again, it's the same transition, changed.
    event_id TEXT NOT NULL,
    -- The device that played it: as the app says, or the session's device name.
    device TEXT NOT NULL,
    -- The records it went from and into. No reference: a transition outlives its files.
    from_track_id INTEGER NOT NULL,
    to_track_id INTEGER NOT NULL,
    -- The app's id for the listen it led into, as in music_plays.event_id, when it said.
    play_event_id TEXT,
    -- Milliseconds since the epoch, when the mix began.
    started_at INTEGER NOT NULL,
    -- The plan as played: its kind and length, the planner's style, whether it was played
    -- part by part (with lanes) and the key shifts.
    kind TEXT NOT NULL CHECK (kind IN ('beatmatch', 'ramp', 'throw', 'crossfade')),
    seconds REAL NOT NULL CHECK (seconds > 0),
    style TEXT,
    by_parts INTEGER NOT NULL CHECK (by_parts IN (0, 1)),
    band_semitones INTEGER NOT NULL DEFAULT 0,
    voice_semitones INTEGER NOT NULL DEFAULT 0,
    -- The planner's mixability for the pair (0 to 1), when the app had it.
    score REAL CHECK (score BETWEEN 0 AND 1),
    -- Whether the listener skipped while it played, and how far into it.
    skipped INTEGER NOT NULL CHECK (skipped IN (0, 1)),
    skipped_at_seconds REAL CHECK (skipped_at_seconds >= 0),
    CHECK (skipped = 1 OR skipped_at_seconds IS NULL),
    UNIQUE (user_id, event_id)
) STRICT;
CREATE INDEX music_transitions_recent ON music_transitions (user_id, started_at);
