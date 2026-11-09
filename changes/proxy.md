### Added

- A proxy for chosen services, under Settings → Outside services → Proxy: SOCKS5 (names
  looked up by the proxy), HTTP or HTTPS, with an optional login whose password is only ever
  said to be set. Each service (TMDB, MusicBrainz, the Cover Art Archive, Wikimedia, LRCLIB,
  OpenSubtitles, OpenRouter, IMDb and Rotten Tomatoes, LibreTranslate, the identity provider,
  Deluge, Sonarr, Radarr, Lidarr, Prowlarr, slskd and FlareSolverr) goes direct until it's
  switched on there. One switched on never falls back to going direct: while the proxy is
  down its requests fail, naming the proxy, and System's status says so (`proxy_down`). Test
  shows the address the outside world sees, asked of api.ipify.org through the proxy. Changes
  apply without a restart. Indexers and the seedbox's SFTP still go direct.
