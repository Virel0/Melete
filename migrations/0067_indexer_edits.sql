-- Turning an indexer off and renaming it in Melete. `enabled` stays what the source
-- says (Prowlarr's switch; always on for Melete's own trackers), and a Prowlarr import still
-- sets it and the name. These two record an admin's own choice, which an import leaves alone:
-- an indexer is searched only when it's enabled and not turned off here.
ALTER TABLE indexers ADD COLUMN turned_off INTEGER NOT NULL DEFAULT 0
    CHECK (turned_off IN (0, 1));
ALTER TABLE indexers ADD COLUMN renamed INTEGER NOT NULL DEFAULT 0
    CHECK (renamed IN (0, 1));
