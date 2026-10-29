-- Measurements of each music track's audio, for Aoide (decision 47): where
-- its sound starts and stops, and how loud and how fast it is. Taken from one decode after a
-- scan finds the track, or sent by a client that measured it with the same algorithm, and
-- kept until the file changes. Two tables, as in the contract: a client can send one without
-- the other.
CREATE TABLE music_sound_bounds (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    -- The file's modified_at when measured; any other value means the file changed.
    modified_at INTEGER NOT NULL,
    -- Which version of Melete's analysis measured it: a newer one measures server rows again.
    analyzer INTEGER NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('server', 'client')),
    -- Why it couldn't be measured. Not tried again until the file or the analysis changes.
    error TEXT,
    -- Both NULL when there's nothing worth trimming.
    sound_start_ms INTEGER CHECK (sound_start_ms >= 0),
    sound_end_ms INTEGER,
    measured_at INTEGER NOT NULL,
    CHECK ((sound_start_ms IS NULL) = (sound_end_ms IS NULL)),
    CHECK (sound_end_ms > sound_start_ms),
    CHECK (error IS NULL OR sound_start_ms IS NULL)
) STRICT;

CREATE TABLE music_audio_analysis (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    modified_at INTEGER NOT NULL,
    analyzer INTEGER NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('server', 'client')),
    error TEXT,
    -- Integrated loudness by EBU R128 and true peak, together or not at all.
    loudness_lufs REAL CHECK (loudness_lufs <= 0),
    true_peak_dbfs REAL,
    -- Kept even when unsure: the confidence threshold is applied when serving, so it can be
    -- reconsidered without measuring again. Stability is NULL for a track too short to tell.
    bpm REAL CHECK (bpm > 0),
    bpm_confidence REAL CHECK (bpm_confidence BETWEEN 0 AND 1),
    bpm_stability REAL CHECK (bpm_stability BETWEEN 0 AND 1),
    measured_at INTEGER NOT NULL,
    CHECK ((loudness_lufs IS NULL) = (true_peak_dbfs IS NULL)),
    CHECK ((bpm IS NULL) = (bpm_confidence IS NULL)),
    CHECK (bpm IS NOT NULL OR bpm_stability IS NULL),
    CHECK (error IS NULL OR (loudness_lufs IS NULL AND bpm IS NULL))
) STRICT;
