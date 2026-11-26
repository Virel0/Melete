-- Waiting for everyone: a player loading or buffering says so, and a playing party pauses for
-- it where it is, carrying on once everyone's ready or after 30 seconds.
ALTER TABLE watch_parties ADD COLUMN held INTEGER NOT NULL DEFAULT 0 CHECK (held IN (0, 1));

CREATE TABLE watch_party_waits (
    party_id INTEGER NOT NULL REFERENCES watch_parties (id) ON DELETE CASCADE,
    -- Who: 'd' and a household device's sign-in id, or 'g' and a guest's id.
    waiter TEXT NOT NULL CHECK (waiter GLOB '[dg][0-9]*'),
    -- The name shown while the party waits.
    name TEXT NOT NULL,
    since INTEGER NOT NULL,
    PRIMARY KEY (party_id, waiter)
) STRICT;
