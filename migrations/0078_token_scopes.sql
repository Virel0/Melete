-- What an API key may reach: read (browsing and reading one's own data), playback (that, plus
-- playing and reporting progress), music-sync (Aoide's routes alone) or full (everything its
-- user can). Keys made before keep full, as they've always had. Sessions are always full.
ALTER TABLE tokens ADD COLUMN scope TEXT NOT NULL DEFAULT 'full'
    CHECK (scope IN ('read', 'playback', 'music-sync', 'full') AND (scope = 'full' OR kind = 'api_key'));
