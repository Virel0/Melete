-- Stalled torrents: when a download's progress last went up (NULL: not since it was added), when
-- it was last found stalled with nothing else to grab, and what was said then, kept beside
-- Deluge's state until it moves again.
ALTER TABLE downloads ADD COLUMN progress_at INTEGER;
ALTER TABLE downloads ADD COLUMN stall_checked_at INTEGER;
ALTER TABLE downloads ADD COLUMN stall_note TEXT;
