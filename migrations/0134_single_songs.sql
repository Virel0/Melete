-- When a wanted single or EP had its songs compared with its artist's albums' (#212): NULL until
-- it has. Albums wanted before this, and any asked for one by one, aren't compared.
ALTER TABLE wanted_albums ADD COLUMN songs_checked_at INTEGER;
UPDATE wanted_albums SET songs_checked_at = strftime('%s', 'now');
