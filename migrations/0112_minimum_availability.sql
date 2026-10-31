-- How far out a film must be before a movie profile takes its releases: announced (any time),
-- in_cinemas (from its cinema release) or released (from its digital or disc release). Series
-- profiles keep the default and don't use it.
ALTER TABLE quality_profiles ADD COLUMN minimum_availability TEXT NOT NULL DEFAULT 'released'
    CHECK (minimum_availability IN ('announced', 'in_cinemas', 'released'));
