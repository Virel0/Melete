-- Melete's key for Gluetun's control server (MELETE_VPN_KEY) is a secret setting like the
-- others: stored sealed once the database is switched. The two triggers are made again with it
-- in their list, which a test holds to the catalogue's.

DROP TRIGGER settings_secret_sealed_insert;
DROP TRIGGER settings_secret_sealed_update;

CREATE TRIGGER settings_secret_sealed_insert BEFORE INSERT ON settings
WHEN NEW.name IN ('MELETE_TMDB_TOKEN', 'MELETE_LIBRETRANSLATE_API_KEY', 'MELETE_SLSKD_API_KEY',
        'MELETE_DELUGE_PASSWORD', 'MELETE_SEEDBOX_PASSWORD',
        'MELETE_OIDC_CLIENT_SECRET', 'MELETE_OPENROUTER_KEY', 'MELETE_OPENSUBTITLES_KEY',
        'MELETE_OPENSUBTITLES_PASSWORD', 'MELETE_PROXY_PASSWORD', 'MELETE_VPN_KEY')
    AND substr(NEW.value, 1, 10) != 'sealed:v1:'
    AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'secret settings are stored sealed');
END;

CREATE TRIGGER settings_secret_sealed_update BEFORE UPDATE OF name, value ON settings
WHEN NEW.name IN ('MELETE_TMDB_TOKEN', 'MELETE_LIBRETRANSLATE_API_KEY', 'MELETE_SLSKD_API_KEY',
        'MELETE_DELUGE_PASSWORD', 'MELETE_SEEDBOX_PASSWORD',
        'MELETE_OIDC_CLIENT_SECRET', 'MELETE_OPENROUTER_KEY', 'MELETE_OPENSUBTITLES_KEY',
        'MELETE_OPENSUBTITLES_PASSWORD', 'MELETE_PROXY_PASSWORD', 'MELETE_VPN_KEY')
    AND substr(NEW.value, 1, 10) != 'sealed:v1:'
    AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'secret settings are stored sealed');
END;
