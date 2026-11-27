### Added

- Invite through the tunnel, in the player's Together panel while Melete's own tunnel is on:
  one guest outside the household gets a WireGuard config of their own (to download or copy)
  and the link to open once it's on, with the three steps to send them. It stays shown while
  the panel follows the party, until Done.
- `compose.tls.yaml`, for HTTPS served by Melete itself with a certificate from a folder of
  your own (`MELETE_TLS_PORT`, `MELETE_TLS_CERT_DIR`, `MELETE_TLS_PROXY_PROTOCOL`): before,
  HTTPS needed a compose file of your own. The remote access guide has a new section on a VPS
  passing HTTPS through to Melete, unread, with the PROXY protocol saying who each client is.
- Search finds My devices and Alerts, and the Tunnel page's empty state leads to My devices.

### Fixed

- Text boxes (a device's WireGuard config) look like the other fields, on dark backgrounds too.
