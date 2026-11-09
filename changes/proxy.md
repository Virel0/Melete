### Added

- A proxy for chosen services, under Settings → Outside services → Proxy: SOCKS5 (names
  looked up by the proxy), HTTP or HTTPS, with an optional login whose password is only ever
  said to be set. Each service (TMDB, MusicBrainz, the Cover Art Archive, Wikimedia, LRCLIB,
  OpenSubtitles, OpenRouter, IMDb and Rotten Tomatoes, LibreTranslate, the identity provider,
  Deluge, the seedbox's SFTP, Sonarr, Radarr, Lidarr, Prowlarr, slskd and FlareSolverr) goes
  direct until it's switched on there. One switched on never falls back to going direct: while the proxy is
  down its requests fail, naming the proxy, and System's status says so (`proxy_down`). Test
  shows the address the outside world sees, asked of api.ipify.org through the proxy. Changes
  apply without a restart. Each indexer has its own switch in the indexer list, and the
  torrents its results link to are fetched the same way; Uindex's pages go through FlareSolverr,
  which (from version 3.2.0) is handed the proxy's address, never a password, or isn't asked
  at all. The seedbox's SFTP goes through a SOCKS5 proxy (which looks the name up) or an
  HTTP or HTTPS one that lets it CONNECT to the SSH port, and `melete seedbox host-key`
  follows the same switch. A copy from the seedbox already running when the switch changes
  finishes the way it started; the next one follows it.
