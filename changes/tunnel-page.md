### Added

- A Tunnel page for admins under Manage → System: every peer on Melete's own WireGuard tunnel
  now, household devices and watch party guests, with whose it is, whether it's connected, its
  last handshake, data in and out, open connections, what it's doing (watching, listening, in
  a party, downloading or browsing) and, only while it's connected, the outside address it
  connects from. It updates every few seconds while open, and Remove cuts a peer off at once.
  The same is in the API (`GET /api/v1/tunnel/peers`, `DELETE /api/v1/tunnel/peers/{address}`,
  admins only).
