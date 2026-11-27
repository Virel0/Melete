-- Household devices on Melete's own WireGuard tunnel (`tunnel`): each a user's, named, with
-- the public key the device signs its handshakes with (Melete never keeps a device's private
-- key), the preshared key Melete made for it (sealed as other secrets are once the database is
-- switched, and refused plain then), its fixed address in the tunnel (an IPv4 address as a
-- number, kept across restarts), and the parts of Melete it reaches besides sign-in and the
-- web page. A user has at most 10; deleting the user deletes their devices.
CREATE TABLE tunnel_devices (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 60 AND name = trim(name)),
    public_key BLOB NOT NULL UNIQUE CHECK (length(public_key) = 32),
    preshared_key BLOB NOT NULL CHECK (length(preshared_key) >= 32),
    address INTEGER NOT NULL UNIQUE CHECK (address BETWEEN 0 AND 4294967295),
    video INTEGER NOT NULL CHECK (video IN (0, 1)),
    music INTEGER NOT NULL CHECK (music IN (0, 1)),
    requests INTEGER NOT NULL CHECK (requests IN (0, 1)),
    admin INTEGER NOT NULL CHECK (admin IN (0, 1)),
    created_at INTEGER NOT NULL,
    CHECK (video + music + requests + admin >= 1)
) STRICT;

CREATE INDEX tunnel_devices_user ON tunnel_devices (user_id);

CREATE TRIGGER tunnel_devices_per_user BEFORE INSERT ON tunnel_devices
WHEN (SELECT count(*) FROM tunnel_devices WHERE user_id = NEW.user_id) >= 10
BEGIN
    SELECT RAISE(ABORT, 'a user has at most 10 tunnel devices');
END;

-- A device is removed and added again rather than changed: its key and address are its own.
CREATE TRIGGER tunnel_devices_fixed BEFORE UPDATE OF user_id, public_key, address
ON tunnel_devices
BEGIN
    SELECT RAISE(ABORT, 'a tunnel device''s user, key and address are fixed');
END;

CREATE TRIGGER tunnel_devices_sealed_insert BEFORE INSERT ON tunnel_devices
WHEN length(NEW.preshared_key) = 32 AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'a tunnel device''s preshared key is stored sealed');
END;

CREATE TRIGGER tunnel_devices_sealed_update BEFORE UPDATE OF preshared_key ON tunnel_devices
WHEN length(NEW.preshared_key) = 32 AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'a tunnel device''s preshared key is stored sealed');
END;
