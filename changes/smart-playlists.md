### Added

- Smart playlists worked out on the server, in the Aoide apps' rule format, from the library and
  the listener's own likes, flags and listens on every device and the web:
  `GET /playlists/{id}/songs` gives a smart playlist's songs, leaving out songs marked Not
  interested.
