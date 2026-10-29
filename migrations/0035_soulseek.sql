-- Soulseek through slskd: an album search also asks slskd, and a result found
-- there is one user's album folder.

-- How asking slskd went: {"results", "error"}; NULL when it wasn't asked.
ALTER TABLE searches ADD COLUMN soulseek TEXT CHECK (soulseek IS NULL OR json_valid(soulseek));

-- A Soulseek result's peer, folder and files to fetch: {"username", "folder", "files":
-- [{"filename", "size"}]}. NULL for a torrent.
ALTER TABLE release_decisions ADD COLUMN soulseek TEXT
    CHECK (soulseek IS NULL OR json_valid(soulseek));
