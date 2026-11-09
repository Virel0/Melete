### Added

- A VPN beside Melete, `compose.vpn.yaml`: Gluetun with its HTTP proxy on, shared by Soulseek,
  the trailer downloader and the services sent through the proxy. Settings → Outside services →
  VPN shows whether it's running, the address the outside world sees and the forwarded port,
  starts, stops and restarts it, and keeps the server countries chosen. Melete reaches its
  control server with a key of its own, stored sealed and never shown, and refuses one that
  answers without a key. The VPN's own keys stay with Gluetun.

### Changed

- Gluetun moved from `compose.soulseek.yaml` to `compose.vpn.yaml`, with a roles file giving
  slskd and Melete a control-server key each (`gluetun-roles.example.toml`). To upgrade, add
  `compose.vpn.yaml` to `COMPOSE_FILE` before `compose.soulseek.yaml` or
  `compose.trailers.yaml`, and set `MELETE_GLUETUN_PROXY_PASSWORD` and
  `MELETE_GLUETUN_ROLES_FILE` in `.env`, with slskd's role keyed by `MELETE_GLUETUN_CONTROL_KEY`.
