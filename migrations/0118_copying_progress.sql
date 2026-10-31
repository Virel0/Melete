-- How far a download's copy home has got, written while it's copied (at most once a second, and
-- at the start and end of each file): bytes copied so far, counting what an earlier copy that was
-- cut off left; bytes to copy in all; and the speed of this copy in bytes a second, once it has
-- run a second. Shown only while the download is being copied home.
ALTER TABLE downloads ADD COLUMN copied INTEGER CHECK (copied IS NULL OR copied >= 0);
ALTER TABLE downloads ADD COLUMN copy_total INTEGER CHECK (copy_total IS NULL OR copy_total >= 0);
ALTER TABLE downloads ADD COLUMN copy_rate INTEGER CHECK (copy_rate IS NULL OR copy_rate > 0);
