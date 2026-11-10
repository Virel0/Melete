-- Playlist imports' tracks left unmatched are handled again: matched with what MusicBrainz now
-- allows (the ISRC lookup it refused), and asked for by name when it has no recording that
-- fits. Their imports are queued to work through them; tracks already asked for are untouched.
UPDATE playlist_import_tracks SET state = 'pending', why = NULL, request_id = NULL
WHERE state = 'unmatched';
UPDATE playlist_imports SET state = 'queued', message = NULL, finished_at = NULL
WHERE id IN (SELECT import_id FROM playlist_import_tracks WHERE state = 'pending');
