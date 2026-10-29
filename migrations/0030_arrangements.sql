-- Arrangements for Aoide (decision 48): sections, phrase grid and vocal spans,
-- from the same decode as the measurements in 0028 and 0029. A track without a row here is
-- measured again, which is how tracks measured before this get theirs.
CREATE TABLE music_arrangements (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    -- The file's modified_at when measured, and which version of the analysis measured it.
    modified_at INTEGER NOT NULL,
    analyzer INTEGER NOT NULL,
    -- Why it couldn't be measured. Not tried again until the file or the analysis changes.
    error TEXT,
    -- [{startMs, endMs, kind, energy}] as served; NULL when there's no structure worth
    -- describing (no beat grid, or too short).
    sections TEXT CHECK (json_valid(sections) AND json_type(sections) = 'array'),
    phrase_bars INTEGER CHECK (phrase_bars IN (8, 16, 32)),
    phrase_anchor_ms REAL,
    -- [{startMs, endMs}] where someone is probably singing: '[]' when nobody was found, NULL
    -- when it couldn't be told. The two must never be confused.
    vocals TEXT CHECK (json_valid(vocals) AND json_type(vocals) = 'array'),
    measured_at INTEGER NOT NULL,
    CHECK ((phrase_bars IS NULL) = (phrase_anchor_ms IS NULL)),
    CHECK (sections IS NOT NULL OR (phrase_bars IS NULL AND vocals IS NULL)),
    CHECK (error IS NULL OR sections IS NULL)
) STRICT;
