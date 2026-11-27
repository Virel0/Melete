-- Openings and endings told apart: each distinct one a series has (a new season's new opening,
-- a change halfway through one) gets an id, kept with a short fingerprint of its start so the
-- same song in another episode, of any season, is given the same id. Who has seen which is
-- kept per user, so a player skips by itself only what that user has heard before.
CREATE TABLE segment_themes (
    id INTEGER PRIMARY KEY,
    series_id INTEGER NOT NULL REFERENCES series (id),
    kind TEXT NOT NULL CHECK (kind IN ('intro', 'credits')),
    -- Its first 60 seconds at most, as little-endian 32-bit hashes (1875 of them).
    fingerprint BLOB NOT NULL
        CHECK (length(fingerprint) BETWEEN 4 AND 7500 AND length(fingerprint) % 4 = 0),
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX segment_themes_series ON segment_themes (series_id, kind);

-- NULL for a segment found before themes were told apart, or one too short to tell.
ALTER TABLE media_segments ADD COLUMN theme_id INTEGER REFERENCES segment_themes (id);

CREATE TABLE segment_themes_seen (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    theme_id INTEGER NOT NULL REFERENCES segment_themes (id),
    seen_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, theme_id)
) STRICT;
