-- Each user's profile picture: their own upload, or a copy of one picked from the gallery, so
-- it stays when the gallery changes. Served by the SHA-256 of its bytes, which the address
-- carries so a new picture isn't cached as the old.
CREATE TABLE user_avatars (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    image_hash TEXT NOT NULL CHECK (length(image_hash) = 64),
    mime_type TEXT NOT NULL CHECK (mime_type IN ('image/jpeg', 'image/png', 'image/webp')),
    bytes BLOB NOT NULL,
    size INTEGER NOT NULL CHECK (size > 0 AND size <= 5242880),
    updated_at INTEGER NOT NULL
) STRICT;
