-- Each probed file version's chapters (start, end, title) as JSON; NULL until the file is read
-- for them, so files probed before are probed once more.
ALTER TABLE media_probes ADD COLUMN chapters TEXT;
