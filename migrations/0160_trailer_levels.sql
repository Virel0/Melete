-- When a downloaded trailer's sound was levelled (to the same loudness as every other), and
-- what came of it: levelled, already at that loudness, no sound, or why it couldn't be (the
-- file is kept as it was). NULL until it's been done, as for every trailer downloaded before.
ALTER TABLE downloaded_trailers ADD COLUMN levelled_at INTEGER;
ALTER TABLE downloaded_trailers ADD COLUMN level_note TEXT;
