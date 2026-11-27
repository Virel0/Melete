-- The language a profile needs subtitles in when a download's audio isn't in it: a download
-- with neither is set aside at import, blocklisted and searched again. Anime profiles start
-- with English, as their releases are mostly Japanese audio and raws carry no subtitles.
ALTER TABLE quality_profiles ADD COLUMN subtitles_language TEXT;
UPDATE quality_profiles SET subtitles_language = 'en' WHERE service = 'anime';
