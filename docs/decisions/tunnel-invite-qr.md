Decision: A tunnel invite's answer carries its WireGuard config as a QR code too (`wireguard_config_qr`), an SVG drawn by the server with the `qrcode` crate (SVG output alone, no image library), black on white with its quiet zone, error correction level M. The Together panel shows it under the config only when Show QR code is pressed, with a caption saying it holds the guest's key; it's kept nowhere, like the config. Two-factor setup keeps its own encoder, which stops at version 10 (213 bytes), less than a WireGuard config with its preshared key needs.

Why: Phones' WireGuard apps import a config by scanning it, which is easier than moving a file or pasting text. Drawing it on the server keeps the key from going to anything outside, and the page needs no library for it; shown only when asked, the key isn't on screen for longer than needed.

Date: 2026-11-28
