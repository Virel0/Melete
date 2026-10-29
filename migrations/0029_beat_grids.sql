-- Beat grids and keys for Aoide (decision 48), from the same decode as the
-- measurements in 0028. A track without a row here is measured again, which is how tracks
-- measured before this get theirs.
CREATE TABLE music_beat_grids (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    -- The file's modified_at when measured, and which version of the analysis measured it.
    modified_at INTEGER NOT NULL,
    analyzer INTEGER NOT NULL,
    -- Why it couldn't be measured. Not tried again until the file or the analysis changes.
    error TEXT,
    -- [{startMs, endMs, anchorMs, bpm, residualMs, beats}] as served; NULL when there's no
    -- grid worth having.
    segments TEXT CHECK (json_valid(segments) AND json_type(segments) = 'array'),
    beats_per_bar INTEGER CHECK (beats_per_bar IN (3, 4)),
    downbeat_index INTEGER CHECK (downbeat_index >= 0),
    mix_in_ms REAL,
    mix_out_ms REAL,
    -- Camelot, such as 8A. Measured with or without a grid; served only with one.
    key_camelot TEXT CHECK (key_camelot GLOB '[1-9][AB]' OR key_camelot GLOB '1[0-2][AB]'),
    key_confidence REAL CHECK (key_confidence BETWEEN 0 AND 1),
    measured_at INTEGER NOT NULL,
    CHECK ((key_camelot IS NULL) = (key_confidence IS NULL)),
    CHECK ((beats_per_bar IS NULL) = (downbeat_index IS NULL)),
    CHECK (segments IS NOT NULL
           OR (beats_per_bar IS NULL AND mix_in_ms IS NULL AND mix_out_ms IS NULL)),
    CHECK (error IS NULL OR (segments IS NULL AND key_camelot IS NULL))
) STRICT;
