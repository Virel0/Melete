-- What a watch party plays next, as the host's player counts down to it: the file and when it
-- starts. Cleared when the party moves on.
ALTER TABLE watch_parties ADD COLUMN up_next_file_id INTEGER
    REFERENCES media_files (id) ON DELETE SET NULL;
ALTER TABLE watch_parties ADD COLUMN up_next_at INTEGER;

-- Why a guest is no longer in their party, kept a little while for their event stream's last
-- word: they left, were sent away, or the party ended (the default, when a guest goes with
-- their party). Leaving and being sent away are noted before the guest goes.
CREATE TABLE watch_party_guests_gone (
    guest_id INTEGER PRIMARY KEY,
    party_id INTEGER NOT NULL,
    reason TEXT NOT NULL CHECK (reason IN ('ended', 'sent_away', 'left')),
    at INTEGER NOT NULL
) STRICT;

CREATE TRIGGER watch_party_guest_gone AFTER DELETE ON watch_party_guests
BEGIN
    INSERT OR IGNORE INTO watch_party_guests_gone (guest_id, party_id, reason, at)
    VALUES (OLD.id, OLD.party_id, 'ended',
            CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));
END;
