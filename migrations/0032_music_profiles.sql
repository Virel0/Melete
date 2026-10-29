-- Music quality profiles: which formats an album may come in, and which is
-- better, as in Lidarr. Imported from Lidarr, or the built-in preset that prefers FLAC.
CREATE TABLE music_profiles (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL CHECK (source IN ('lidarr', 'preset')),
    name TEXT NOT NULL,
    upgrade_allowed INTEGER NOT NULL CHECK (upgrade_allowed IN (0, 1)),
    -- Upgrades stop once an album reaches this quality's (or group's) rank.
    cutoff TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (source, name)
) STRICT;
-- The qualities a profile ranks, lowest first; a group's qualities share a rank.
CREATE TABLE music_profile_qualities (
    profile_id INTEGER NOT NULL REFERENCES music_profiles (id) ON DELETE CASCADE,
    quality TEXT NOT NULL CHECK (quality IN (
        'Unknown', 'MP3-008', 'MP3-016', 'MP3-024', 'MP3-032', 'MP3-040', 'MP3-048', 'MP3-056',
        'MP3-064', 'MP3-080', 'MP3-096', 'MP3-112', 'MP3-128', 'MP3-160', 'MP3-192', 'MP3-224',
        'MP3-256', 'MP3-320', 'MP3-VBR-V0', 'MP3-VBR-V2', 'AAC-192', 'AAC-256', 'AAC-320',
        'AAC-VBR', 'WMA', 'OGG Vorbis Q5', 'OGG Vorbis Q6', 'OGG Vorbis Q7', 'OGG Vorbis Q8',
        'OGG Vorbis Q9', 'OGG Vorbis Q10', 'WAV', 'WavPack', 'APE', 'FLAC', 'ALAC',
        'FLAC 24bit', 'ALAC 24bit')),
    rank INTEGER NOT NULL CHECK (rank >= 0),
    -- The group's name ("Lossless") when the quality is ranked with others.
    group_name TEXT,
    allowed INTEGER NOT NULL CHECK (allowed IN (0, 1)),
    PRIMARY KEY (profile_id, quality)
) STRICT;
-- A profile chosen in Melete for a wanted artist; without one, the profile imported from
-- Lidarr under the artist's quality_profile name.
ALTER TABLE wanted_artists ADD COLUMN profile_id INTEGER REFERENCES music_profiles (id) ON DELETE SET NULL;
