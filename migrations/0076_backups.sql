-- Copies of the database, each attempt kept: the one made before migrating, the daily ones and
-- those asked for by hand. The copies themselves are files in the data folder's `backups`;
-- these rows say when each was tried and why one failed, for the admins' health page.
CREATE TABLE backups (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('before_migrating', 'daily', 'by_hand')),
    -- The copy's file name in `backups`, once made; none when it failed.
    file TEXT,
    bytes INTEGER CHECK (bytes >= 0),
    -- Why it failed, in plain words; none when it was made.
    error TEXT,
    made_at INTEGER NOT NULL,
    CHECK ((file IS NULL) = (error IS NOT NULL)),
    CHECK ((file IS NULL) = (bytes IS NULL))
) STRICT;
CREATE INDEX backups_by_kind ON backups (kind, made_at);
