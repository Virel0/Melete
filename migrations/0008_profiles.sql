-- Custom formats and quality profiles, imported from Radarr and Sonarr. Names are
-- unique per app: Radarr's "DV" and Sonarr's "DV" can differ (the apps number sources and
-- languages differently), and each app's profiles score only that app's formats.

CREATE TABLE custom_formats (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL CHECK (source IN ('radarr', 'sonarr')),
    name TEXT NOT NULL,
    -- The conditions as JSON, in the shape of src/profiles/formats.rs; checked when written.
    specifications TEXT NOT NULL CHECK (json_valid(specifications)),
    updated_at INTEGER NOT NULL,
    UNIQUE (source, name)
) STRICT;

CREATE TABLE quality_profiles (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL CHECK (source IN ('radarr', 'sonarr')),
    name TEXT NOT NULL,
    upgrade_allowed INTEGER NOT NULL CHECK (upgrade_allowed IN (0, 1)),
    -- Upgrades stop once a file reaches this quality's rank and cutoff_format_score.
    cutoff TEXT NOT NULL,
    min_format_score INTEGER NOT NULL,
    cutoff_format_score INTEGER NOT NULL,
    min_upgrade_format_score INTEGER NOT NULL,
    -- Radarr's profile language: an ISO 639-1 code, 'original' or 'any'. Sonarr has none.
    language TEXT,
    updated_at INTEGER NOT NULL,
    UNIQUE (source, name)
) STRICT;

-- The qualities a profile ranks, lowest first; a group's qualities share a rank.
CREATE TABLE quality_profile_qualities (
    profile_id INTEGER NOT NULL REFERENCES quality_profiles (id) ON DELETE CASCADE,
    quality TEXT NOT NULL CHECK (quality IN (
        'Unknown', 'WORKPRINT', 'CAM', 'TELESYNC', 'TELECINE', 'REGIONAL', 'DVDSCR', 'SDTV',
        'DVD', 'DVD-R', 'WEBDL-480p', 'WEBRip-480p', 'Bluray-480p', 'Bluray-576p',
        'HDTV-720p', 'WEBDL-720p', 'WEBRip-720p', 'Bluray-720p', 'HDTV-1080p', 'WEBDL-1080p',
        'WEBRip-1080p', 'Bluray-1080p', 'Remux-1080p', 'HDTV-2160p', 'WEBDL-2160p',
        'WEBRip-2160p', 'Bluray-2160p', 'Remux-2160p', 'BR-DISK', 'Raw-HD')),
    rank INTEGER NOT NULL CHECK (rank >= 0),
    -- The group's name ("WEB 1080p") when the quality is ranked with others.
    group_name TEXT,
    allowed INTEGER NOT NULL CHECK (allowed IN (0, 1)),
    PRIMARY KEY (profile_id, quality)
) STRICT;

CREATE TABLE quality_profile_formats (
    profile_id INTEGER NOT NULL REFERENCES quality_profiles (id) ON DELETE CASCADE,
    format_id INTEGER NOT NULL REFERENCES custom_formats (id) ON DELETE CASCADE,
    score INTEGER NOT NULL,
    PRIMARY KEY (profile_id, format_id)
) STRICT;
