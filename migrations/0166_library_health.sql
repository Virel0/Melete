-- Whether each library's folder answers: set when a regular read of it doesn't come back in
-- time, can't be done or finds it empty, and cleared when it answers again.
ALTER TABLE libraries ADD COLUMN unreachable_since INTEGER;
ALTER TABLE libraries ADD COLUMN unreachable_why TEXT;
