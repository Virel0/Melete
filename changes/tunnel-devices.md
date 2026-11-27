### Added

- Household devices on Melete's own WireGuard tunnel: each user adds their own (`POST
  /api/v1/tunnel/devices`, at most 10), lists them (`GET`) and removes them (`DELETE
  /api/v1/tunnel/devices/{id}`, an admin anyone's), and a "My devices" section on the account
  page does the same. A device is kept across restarts with the same address, and reaches
  signing in, its user's own account and the web page, plus the parts it's given (video,
  music, requests, admin), as its own user alone. An app sends only its public key; without
  one, Melete makes the pair and shows the config once. The preshared key is Melete's, kept
  sealed and shown once. Removing a device, or deleting its user, cuts it off at once. See
  docs/REMOTE-ACCESS.md.
