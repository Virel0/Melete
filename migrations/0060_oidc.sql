-- Identities at an outside identity provider, each linked by its user while signed in.
-- An identity signs in only as the user it's linked to; one per user and provider.
CREATE TABLE oidc_identities (
    issuer TEXT NOT NULL CHECK (issuer != ''),
    subject TEXT NOT NULL CHECK (subject != '' AND length(subject) <= 255),
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    linked_at INTEGER NOT NULL,
    PRIMARY KEY (issuer, subject),
    UNIQUE (user_id, issuer)
) STRICT;
