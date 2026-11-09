-- Whether an indexer is reached through the proxy set under Outside services: its searches,
-- its tests, and the torrents its results link to. Off for every indexer, as before; a
-- Prowlarr import leaves it as an admin set it.
ALTER TABLE indexers ADD COLUMN proxy INTEGER NOT NULL DEFAULT 0 CHECK (proxy IN (0, 1));
