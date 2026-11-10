### Added

- Songs from YouTube as a last resort (`MELETE_SONG_YOUTUBE`, off by default, with the trailer
  downloader): a song someone asked for that Soulseek found nothing for, or whose Soulseek
  downloads all failed, is looked for on YouTube and fetched when a video's title, artist and
  length all fit, never a cover, remix or sped-up version. Songs already wanted are tried too,
  the newest first. Their audio is YouTube's, lossy, as it comes.

### Changed

- A song Soulseek was searched for without a fit says so, instead of "waiting for a search".

### Fixed

- A trailer blocked in the VPN's country, or on a channel that refuses VPNs, counts as not on
  YouTube instead of failing three times: the downloader kept yt-dlp's last line, a hint
  printed after the error, so the reason was lost and the block went unrecognised.
