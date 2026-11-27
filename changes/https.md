### Added

- HTTPS served by Melete itself, beside plain HTTP: `MELETE_TLS_BIND` (e.g. `0.0.0.0:8443`),
  with a certificate chain and key from PEM files (`MELETE_TLS_CERT`, `MELETE_TLS_KEY`), read
  again within 10 minutes of being renewed, no restart needed. A proxy in front (a VPS) can pass
  connections through as they are, unable to read them, and say who each client is with the
  PROXY protocol (`MELETE_TLS_PROXY_PROTOCOL=on`), believed only from `MELETE_TRUSTED_PROXIES`.
  For example, nginx on the VPS:

      stream {
          server {
              listen 443;
              proxy_pass 10.20.0.2:8443;   # Melete over the tunnel home
              proxy_protocol on;
          }
      }
