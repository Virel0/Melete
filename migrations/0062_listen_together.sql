-- Listening together: a session that household devices play in step. The server keeps
-- what's playing; each device plays its own stream of it. Devices are sessions (tokens), so a
-- device that signs out leaves, and a session nobody is in ends.
CREATE TABLE listen_sessions (
    id INTEGER PRIMARY KEY,
    -- Shown by those in it to let others join; unique among sessions.
    code TEXT NOT NULL UNIQUE,
    host_user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Track ids, in order, as JSON.
    queue TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(queue) AND json_type(queue) = 'array'),
    -- Which entry of the queue is playing.
    queue_index INTEGER NOT NULL DEFAULT 0 CHECK (queue_index >= 0),
    -- Where in it, at `position_at` (Unix milliseconds); while playing it moves on from there.
    position_ms INTEGER NOT NULL DEFAULT 0 CHECK (position_ms >= 0),
    position_at INTEGER NOT NULL,
    playing INTEGER NOT NULL DEFAULT 0 CHECK (playing IN (0, 1)),
    -- Raised by every change, so two devices changing it at once can't undo each other.
    version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE listen_members (
    session_id INTEGER NOT NULL REFERENCES listen_sessions (id) ON DELETE CASCADE,
    token_id INTEGER NOT NULL REFERENCES tokens (id) ON DELETE CASCADE,
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (session_id, token_id)
) STRICT;

CREATE INDEX listen_members_token ON listen_members (token_id);
