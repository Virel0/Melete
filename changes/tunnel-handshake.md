### Fixed

- The Tunnel page no longer says a device that connected "Never connected" once it goes quiet.
  WireGuard forgets a quiet peer's session after a few minutes, and with it its last
  handshake; Melete now remembers the last one it saw (or the peer's last request, which came
  over one), and says "Never connected" only for a peer that has exchanged nothing at all.
