### Added

- A tunnel invite's WireGuard config can be shown as a QR code (Show QR code, under the config),
  for a phone's WireGuard app to scan. The server draws it as SVG, so nothing is sent anywhere
  to make it; it holds the guest's key like the config, and is shown only when asked.
