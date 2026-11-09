### Added

- A VPN beside Melete, `compose.vpn.yaml`: Gluetun with its HTTP proxy on, shared by Soulseek,
  the trailer downloader and the services sent through the proxy. Settings → Outside services →
  VPN shows whether it's running, the address the outside world sees and the forwarded port,
  starts, stops and restarts it. Which provider and servers it uses stays in Gluetun's own
  settings file (`SERVER_COUNTRIES` and the rest), which Melete never reads or changes. Melete
  reaches its control server with a key of its own, stored sealed and never shown, and refuses
  one that answers a request without a key. The VPN's own keys stay with Gluetun, and its
  proxy adds no header naming Melete's address.

### Changed

- Gluetun moved from `compose.soulseek.yaml` to `compose.vpn.yaml`, with a roles file giving
  slskd and Melete a control-server key each (`gluetun-roles.example.toml`). To upgrade, add
  `compose.vpn.yaml` to `COMPOSE_FILE` before `compose.soulseek.yaml` or
  `compose.trailers.yaml`, and set `MELETE_GLUETUN_PROXY_PASSWORD` and
  `MELETE_GLUETUN_ROLES_FILE` in `.env`, with slskd's role keyed by `MELETE_GLUETUN_CONTROL_KEY`.
  The example roles file has empty keys, and Gluetun won't start until each role has one.
  Gluetun's proxy logs a login it turns down, password included, so when changing
  `MELETE_GLUETUN_PROXY_PASSWORD`, change it in Melete's Proxy settings first.
