-- Music quality profiles changed in Melete: when an admin last did, so an import from
-- Lidarr leaves the profile as the admin set it. Putting the preset back as it ships clears it.
ALTER TABLE music_profiles ADD COLUMN edited_at INTEGER;
