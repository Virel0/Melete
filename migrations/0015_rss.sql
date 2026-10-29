-- RSS sync: every indexer's newest releases, checked against the wanted list on a
-- schedule. Releases naming a wanted title are decided on and kept like a search's (decision 9).

-- One run: each indexer asked, how many releases were new, and what was grabbed.
CREATE TABLE rss_syncs (
    id INTEGER PRIMARY KEY,
    synced_at INTEGER NOT NULL,
    -- [{"indexer_id", "name", "results", "error"}], as for searches.
    indexers TEXT NOT NULL CHECK (json_valid(indexers)),
    new_releases INTEGER NOT NULL,
    -- [{"release_id", "title", "download_id", "refused"}]: one of the last two is null.
    grabbed TEXT NOT NULL CHECK (json_valid(grabbed))
) STRICT;

-- The searches an RSS sync stored, one per wanted target its new releases named.
ALTER TABLE searches ADD COLUMN rss_sync_id INTEGER REFERENCES rss_syncs (id) ON DELETE CASCADE;

-- Releases already looked at, so each is decided on once. The key is a hash of the guid (or
-- of the title, without one): guids can carry a tracker passkey.
CREATE TABLE rss_seen (
    indexer_id INTEGER NOT NULL REFERENCES indexers (id) ON DELETE CASCADE,
    release_key TEXT NOT NULL,
    seen_at INTEGER NOT NULL,
    PRIMARY KEY (indexer_id, release_key)
) STRICT;
