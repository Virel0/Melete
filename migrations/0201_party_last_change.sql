-- Who last played, paused, jumped or moved a watch party on, and when: a household member by
-- their account (none once it's gone), a guest by the name they gave, kept as it was.
ALTER TABLE watch_parties ADD COLUMN last_user_id INTEGER REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE watch_parties ADD COLUMN last_guest_id INTEGER
    REFERENCES watch_party_guests (id) ON DELETE SET NULL;
ALTER TABLE watch_parties ADD COLUMN last_guest_name TEXT;
ALTER TABLE watch_parties ADD COLUMN last_kind TEXT
    CHECK (last_kind IN ('play', 'pause', 'seek', 'next'));
ALTER TABLE watch_parties ADD COLUMN last_at INTEGER;
