-- The episodes of its season a season pack was taken for, when part of the season was on disk or
-- on its way already: only the pack's files for these episodes are brought home. NULL takes the
-- whole download, as before.
ALTER TABLE downloads ADD COLUMN episodes TEXT CHECK (episodes IS NULL OR json_valid(episodes));
