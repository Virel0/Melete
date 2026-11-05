### Added

- **Trailer downloads** (off by default): each movie's, series' and anime's trailer, picked
  from TMDB's list of its videos (official, in the title's own language, then the metadata
  language, then English), downloaded once from YouTube by a new trailer downloader
  (`compose.trailers.yaml`, the `-trailers` image) that runs behind `compose.soulseek.yaml`'s
  VPN and won't start without it. Converted once on the GPU to HEVC (HDR kept) unless it's
  already H.264 or HEVC, at most the quality set in Settings (4K, 1080p or 720p), and kept in
  the config folder while the title is in the library; a title with a trailer in its folder
  gets none. Progress shows in Tasks. Playing them comes in a later change.
