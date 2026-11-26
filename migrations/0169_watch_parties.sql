-- Watching together: a party of devices playing one film or episode in step. The server keeps
-- what plays, where and whether it plays; each device plays its own stream of it, with its own
-- audio and subtitles.
CREATE TABLE watch_parties (
    id INTEGER PRIMARY KEY,
    -- Shown by those in it to let others join; unique among parties.
    code TEXT NOT NULL UNIQUE,
    -- Who started it: the one who changes what plays, and who ends it for everyone.
    host_user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
    -- Where in it, at `position_at` (Unix milliseconds); while playing it moves on from there.
    position_ms INTEGER NOT NULL DEFAULT 0 CHECK (position_ms >= 0),
    position_at INTEGER NOT NULL,
    playing INTEGER NOT NULL DEFAULT 0 CHECK (playing IN (0, 1)),
    -- Raised by every change, so two devices changing it at once can't undo each other.
    version INTEGER NOT NULL DEFAULT 1,
    -- Guests' streams are held to this many bits a second when set; household members' never.
    guest_max_bitrate INTEGER CHECK (guest_max_bitrate > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- The devices (sign-in sessions) in each party; a device that signs out leaves.
CREATE TABLE watch_party_members (
    party_id INTEGER NOT NULL REFERENCES watch_parties (id) ON DELETE CASCADE,
    token_id INTEGER NOT NULL REFERENCES tokens (id) ON DELETE CASCADE,
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (party_id, token_id)
) STRICT;
CREATE INDEX watch_party_members_token ON watch_party_members (token_id);
