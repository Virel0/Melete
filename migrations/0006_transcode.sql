-- A stream link can also carry transcoding settings: its token then also opens an HLS
-- playlist of the file re-encoded with them. NULL for direct streams.
ALTER TABLE play_sessions ADD COLUMN transcode TEXT;
