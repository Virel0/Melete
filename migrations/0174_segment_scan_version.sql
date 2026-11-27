-- Which way of finding intros and credits looked at a file: a file looked at an older way is
-- looked at again in the background, while what was found then is still served. Scans before
-- this were the first way.
ALTER TABLE media_segment_scans ADD COLUMN version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1);
