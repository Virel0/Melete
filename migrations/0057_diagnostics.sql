-- Reports apps send about themselves, kept per device for 30 days and at most 100 a
-- device. The device is the session (or API key) that sent it, by name, so a report outlives a
-- revoked session until its time is up.
CREATE TABLE diagnostics (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_id INTEGER REFERENCES tokens (id) ON DELETE SET NULL,
    device TEXT NOT NULL,
    app TEXT NOT NULL,
    app_version TEXT,
    platform TEXT,
    level TEXT NOT NULL CHECK (level IN ('info', 'warning', 'error')),
    summary TEXT NOT NULL,
    -- A JSON object, with anything that looks like a secret replaced.
    details TEXT NOT NULL,
    received_at INTEGER NOT NULL
) STRICT;

CREATE INDEX diagnostics_device ON diagnostics (user_id, device, received_at);
CREATE INDEX diagnostics_received ON diagnostics (received_at);
