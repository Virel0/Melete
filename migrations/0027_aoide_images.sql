-- Playlist artwork for the aoide-sidecar's contract: blobs addressed by the
-- SHA-256 of their bytes, per user, so re-uploading is a no-op and two playlists sharing art
-- keep one copy. Ops carry only the hash.
CREATE TABLE aoide_images (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    image_hash TEXT NOT NULL CHECK (length(image_hash) = 64),
    mime_type TEXT NOT NULL CHECK (mime_type IN ('image/jpeg', 'image/png', 'image/webp')),
    bytes BLOB NOT NULL,
    size INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, image_hash)
) STRICT;
