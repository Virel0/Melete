-- HTTPS's certificate got by Melete from an ACME CA.
--
-- One account per CA directory (the staging server's apart from the real one): its URI, shown
-- for a CAA record's accounturi, and its credentials (the private key, made from the OS's
-- randomness, with the URI and directory) as JSON, sealed as other secrets are once the
-- database is switched, and refused plain then. The contact address it was made or last
-- changed with, empty for none.
CREATE TABLE acme_accounts (
    directory TEXT PRIMARY KEY CHECK (directory LIKE 'https://%' OR directory LIKE 'http://%'),
    uri TEXT NOT NULL CHECK (uri LIKE 'https://%' OR uri LIKE 'http://%'),
    credentials TEXT NOT NULL CHECK (length(credentials) > 0),
    contact TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TRIGGER acme_accounts_sealed_insert BEFORE INSERT ON acme_accounts
WHEN substr(NEW.credentials, 1, 10) != 'sealed:v1:' AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'an ACME account''s credentials are stored sealed');
END;

CREATE TRIGGER acme_accounts_sealed_update BEFORE UPDATE OF credentials ON acme_accounts
WHEN substr(NEW.credentials, 1, 10) != 'sealed:v1:' AND EXISTS (SELECT 1 FROM secrets_sealed)
BEGIN
    SELECT RAISE(ABORT, 'an ACME account''s credentials are stored sealed');
END;

-- Where renewing the certificate stands, kept across restarts: the CA directory the
-- certificate in use came from, when to renew it (in the window the CA's renewal information
-- suggests, else two-thirds through its life) and when to ask the CA about that again, and,
-- after attempts that failed, how many in a row, when to try next, why the last one failed,
-- which CA directory and domain they were for (a change of either starts afresh), and when
-- admins were last alerted; and when an admin last asked for a certificate now, which is
-- refused again for a while. Times are Unix seconds.
CREATE TABLE acme_renewal (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    directory TEXT,
    renew_at INTEGER,
    ask_again_at INTEGER,
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    retry_at INTEGER,
    last_error TEXT CHECK (last_error IS NULL OR length(last_error) <= 1000),
    failed_for TEXT,
    alerted_at INTEGER,
    asked_at INTEGER,
    updated_at INTEGER NOT NULL,
    CHECK ((failures = 0) = (retry_at IS NULL))
) STRICT;

-- Challenge values Melete has set in a TXT record and not yet removed, with where (deSEC's
-- API, the zone, the record's name), kept before they're set: after a crash, these and no
-- others are taken out again. The values are JSON, a list of strings.
CREATE TABLE acme_challenge_left (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    api TEXT NOT NULL,
    zone TEXT NOT NULL,
    subname TEXT NOT NULL,
    ours TEXT NOT NULL CHECK (json_valid(ours) AND json_type(ours) = 'array'),
    set_at INTEGER NOT NULL
) STRICT;
