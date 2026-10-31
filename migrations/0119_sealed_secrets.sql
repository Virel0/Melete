-- Stored secrets sealed with a key kept outside the database (src/secrets.rs): outside
-- services' secret settings, indexers' API keys, two-factor secrets, and search results' links.
--
-- Expand, then contract once switched. Sealing needs the key, which SQL can't see, so the
-- server seals the plain values at its first start after this migration and marks the database
-- switched in `secrets_sealed`, in one transaction. From then on the triggers below refuse a
-- plain value in place of a sealed one; until then (a database the server hasn't started on)
-- they let plain values through, as before.

-- One row once the database is switched; it stays switched.
CREATE TABLE secrets_sealed (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    sealed_at INTEGER NOT NULL,
    -- When the backup copies made before the switch had their plain secrets sealed too; NULL
    -- until every copy was.
    copies_sealed_at INTEGER
) STRICT;

CREATE TRIGGER secrets_sealed_stays BEFORE DELETE ON secrets_sealed
BEGIN
    SELECT RAISE(ABORT, 'a database keeps its secrets sealed once switched');
END;

-- Settings: the catalogue's secrets (Input::Secret), sealed as text starting 'sealed:v1:'. A
-- test holds this list to the catalogue's.
CREATE TRIGGER settings_secret_sealed_insert BEFORE INSERT ON settings
WHEN NEW.name IN ('MELETE_TMDB_TOKEN', 'MELETE_LIBRETRANSLATE_API_KEY', 'MELETE_SLSKD_API_KEY',
        'MELETE_DELUGE_PASSWORD', 'MELETE_SEEDBOX_PASSWORD',
        'MELETE_OIDC_CLIENT_SECRET', 'MELETE_OPENROUTER_KEY', 'MELETE_OPENSUBTITLES_KEY',
        'MELETE_OPENSUBTITLES_PASSWORD')
    AND substr(NEW.value, 1, 10) != 'sealed:v1:'
    AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'secret settings are stored sealed');
END;

CREATE TRIGGER settings_secret_sealed_update BEFORE UPDATE OF name, value ON settings
WHEN NEW.name IN ('MELETE_TMDB_TOKEN', 'MELETE_LIBRETRANSLATE_API_KEY', 'MELETE_SLSKD_API_KEY',
        'MELETE_DELUGE_PASSWORD', 'MELETE_SEEDBOX_PASSWORD',
        'MELETE_OIDC_CLIENT_SECRET', 'MELETE_OPENROUTER_KEY', 'MELETE_OPENSUBTITLES_KEY',
        'MELETE_OPENSUBTITLES_PASSWORD')
    AND substr(NEW.value, 1, 10) != 'sealed:v1:'
    AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'secret settings are stored sealed');
END;

-- Indexers' keys: sealed the same way; empty where a feed needs none.
CREATE TRIGGER indexers_key_sealed_insert BEFORE INSERT ON indexers
WHEN NEW.api_key != '' AND substr(NEW.api_key, 1, 10) != 'sealed:v1:'
    AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'indexer keys are stored sealed');
END;

CREATE TRIGGER indexers_key_sealed_update BEFORE UPDATE OF api_key ON indexers
WHEN NEW.api_key != '' AND substr(NEW.api_key, 1, 10) != 'sealed:v1:'
    AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'indexer keys are stored sealed');
END;

-- Search results' download links and guids, which can carry an indexer's key or a tracker's
-- passkey: sealed the same way.
CREATE TRIGGER release_links_sealed_insert BEFORE INSERT ON release_decisions
WHEN ((NEW.download_url != '' AND substr(NEW.download_url, 1, 10) != 'sealed:v1:')
        OR (NEW.guid != '' AND substr(NEW.guid, 1, 10) != 'sealed:v1:'))
    AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'search results'' links are stored sealed');
END;

CREATE TRIGGER release_links_sealed_update BEFORE UPDATE OF download_url, guid ON release_decisions
WHEN ((NEW.download_url != '' AND substr(NEW.download_url, 1, 10) != 'sealed:v1:')
        OR (NEW.guid != '' AND substr(NEW.guid, 1, 10) != 'sealed:v1:'))
    AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'search results'' links are stored sealed');
END;

-- Two-factor secrets: the table is rebuilt so its secret may be sealed bytes (a version byte
-- 0x01, a 12-byte nonce, the 20-byte secret encrypted and a 16-byte tag: 49 bytes) as well as
-- the plain 20 bytes. Nothing refers to this table, so dropping it touches no other.
CREATE TABLE two_factor_new (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    secret BLOB NOT NULL
        CHECK (length(secret) = 20 OR (length(secret) = 49 AND substr(secret, 1, 1) = x'01')),
    created_at INTEGER NOT NULL,
    -- NULL while setup waits for its first code; sign-in asks for a code only once it's set.
    confirmed_at INTEGER,
    -- The newest 30-second step a code was accepted for; a code is taken once per step, and
    -- never for a step older than this.
    last_step INTEGER
) STRICT;
INSERT INTO two_factor_new (user_id, secret, created_at, confirmed_at, last_step)
SELECT user_id, secret, created_at, confirmed_at, last_step FROM two_factor;
DROP TABLE two_factor;
ALTER TABLE two_factor_new RENAME TO two_factor;

CREATE TRIGGER two_factor_sealed_insert BEFORE INSERT ON two_factor
WHEN length(NEW.secret) = 20 AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'two-factor secrets are stored sealed');
END;

CREATE TRIGGER two_factor_sealed_update BEFORE UPDATE OF secret ON two_factor
WHEN length(NEW.secret) = 20 AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'two-factor secrets are stored sealed');
END;
