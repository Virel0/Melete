-- MusicBrainz matching is off by default, as every outside service is: the starting value 0043
-- saved (updated_at 0) is taken out. A value an admin saved since stays as it is.
DELETE FROM settings WHERE name = 'MELETE_MUSICBRAINZ' AND value = 'on' AND updated_at = 0;
