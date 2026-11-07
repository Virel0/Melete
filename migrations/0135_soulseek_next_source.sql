-- The next source for a Soulseek download that fails once slskd has it: the search its result
-- came from, so the next best result of that search can be grabbed and the search's downloads
-- counted, and, on a download grabbed as the next source, why it's from there, kept before
-- slskd's word on it while it downloads.
ALTER TABLE downloads ADD COLUMN search_id INTEGER REFERENCES searches (id) ON DELETE SET NULL;
ALTER TABLE downloads ADD COLUMN next_source_note TEXT;
CREATE INDEX downloads_by_search ON downloads (search_id);
