### Added

- Melete's own WireGuard tunnel, off unless `MELETE_TUNNEL_PORT` is set: Melete answers
  WireGuard itself, in userspace, on one UDP port, with no TUN device or extra container
  privileges, and serves only its own API through it. A device in a watch party can make an
  invite that comes with a one-off WireGuard config for the guest
  (`POST /api/v1/watch-parties/{id}/tunnel-invites`); through it the guest reaches their party's
  routes and nothing else, and the peer ends with the invite unused, or with the guest's pass.
  `MELETE_TUNNEL_ENDPOINT` says where guests reach the port, and `MELETE_TUNNEL_NET` the range
  inside the tunnel (`10.73.0.0/24` by default). See docs/REMOTE-ACCESS.md.
