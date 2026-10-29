-- The aoide-sidecar's sync store, rewritten into Melete (contract in
-- aoide-sidecar's docs/client-integration.md). Each user's devices push ops to an
-- append-only log and pull everyone else's; payloads are stored as sent and never read here.
-- Track references inside them are Melete track ids.
CREATE TABLE aoide_ops (
    -- AUTOINCREMENT so a pruned sequence number is never handed out again: cursors only move
    -- forward.
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    op_id TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The curation store's device id, as the push said.
    device_id TEXT NOT NULL,
    entity TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    -- The JSON object exactly as pushed.
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    created_at INTEGER NOT NULL,
    received_at INTEGER NOT NULL,
    -- For playlists and their items, so collaborators' edits reach the owner and back.
    playlist_id TEXT,
    UNIQUE (user_id, op_id)
) STRICT;
CREATE INDEX aoide_ops_by_playlist ON aoide_ops (playlist_id) WHERE playlist_id IS NOT NULL;
CREATE INDEX aoide_ops_by_entity ON aoide_ops (user_id, entity, entity_id);

-- How far each device (each session token) has pulled, so retention never trims history a
-- device hasn't read. Only moves forward.
CREATE TABLE aoide_device_cursors (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    cursor INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, device_id)
) STRICT;

-- Who owns each playlist (the first to push it), and who it's shared with.
CREATE TABLE aoide_playlist_owners (
    playlist_id TEXT PRIMARY KEY,
    owner_user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE aoide_playlist_shares (
    playlist_id TEXT NOT NULL,
    grantee_user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    can_edit INTEGER NOT NULL DEFAULT 1 CHECK (can_edit IN (0, 1)),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, grantee_user_id)
) STRICT;
CREATE INDEX aoide_shares_by_grantee ON aoide_playlist_shares (grantee_user_id);
