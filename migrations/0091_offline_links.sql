-- Download links of ready offline copies, as stream links are: their own token (SHA-256
-- stored), a day long, tied to the session that asked, so signing out ends them.
CREATE TABLE offline_links (
    token_hash BLOB PRIMARY KEY,
    copy_id INTEGER NOT NULL REFERENCES offline_copies (id) ON DELETE CASCADE,
    token_id INTEGER NOT NULL REFERENCES tokens (id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
) STRICT;
