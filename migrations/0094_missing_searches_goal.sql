-- What a run searches for: what's missing, or upgrades for files below their profile's cutoff.
-- Both kinds share an area's one run at a time, so together they keep the same pace.
ALTER TABLE missing_searches ADD COLUMN goal TEXT NOT NULL DEFAULT 'missing'
    CHECK (goal IN ('missing', 'cutoff_unmet'));
