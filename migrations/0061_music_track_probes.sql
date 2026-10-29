-- What ffprobe says about music files whose codec the tag reader can't name: E-AC-3 in
-- MP4, Matroska audio, raw E-AC-3 and TrueHD. Kept per version of the file; a file ffprobe
-- rejects keeps its error and isn't asked about again until it changes.
CREATE TABLE music_track_probes (
    track_id INTEGER PRIMARY KEY REFERENCES music_tracks (id) ON DELETE CASCADE,
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    codec TEXT,
    profile TEXT,
    channels INTEGER,
    channel_layout TEXT,
    atmos INTEGER NOT NULL DEFAULT 0 CHECK (atmos IN (0, 1)),
    duration_seconds REAL,
    error TEXT,
    probed_at INTEGER NOT NULL
) STRICT;
