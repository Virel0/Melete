-- Videos YouTube won't give a title's trailer from here (removed, or blocked in this country or
-- to VPNs), passed over when its trailer is chosen, so another of its videos is tried; and the
-- fitting YouTube results a song hasn't tried yet, best first, as [{"id", "title", "channel",
-- "duration"}].
ALTER TABLE downloaded_trailers ADD COLUMN passed_over TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(passed_over));
ALTER TABLE song_youtube ADD COLUMN others TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(others));

-- Trailers that failed for being blocked here before the downloader could tell (it kept yt-dlp's
-- hint to use a VPN, not the error): not on YouTube here, and their video passed over.
UPDATE downloaded_trailers SET state = 'unavailable', attempts = 0, passed_over = json_array(video)
WHERE state = 'failed'
  AND (reason LIKE '%use a VPN or a proxy server%'
       OR reason LIKE '%Unavailable with a VPN or proxy%'
       OR reason LIKE '%available in your country%');
