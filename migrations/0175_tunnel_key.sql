-- The private key of Melete's own WireGuard tunnel (`tunnel`): one, made from the OS's
-- randomness the first time the tunnel starts. Sealed as other secrets are once the database
-- is switched (a sealed key is longer than the 32 bytes of a plain one), and refused plain then.
CREATE TABLE tunnel_key (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    private_key BLOB NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TRIGGER tunnel_key_sealed_insert BEFORE INSERT ON tunnel_key
WHEN length(NEW.private_key) = 32 AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'the tunnel key is stored sealed');
END;

CREATE TRIGGER tunnel_key_sealed_update BEFORE UPDATE OF private_key ON tunnel_key
WHEN length(NEW.private_key) = 32 AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'the tunnel key is stored sealed');
END;
