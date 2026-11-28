### Added

- Certificates can prove the name by the TLS-ALPN challenge instead of DNS: Melete's own HTTPS
  answers Let's Encrypt's `acme-tls/1` connection through the passthrough (after its PROXY
  header), with a certificate made for the check and only while an order waits. No DNS token
  on the server; the certificate is then for the exact name, which CT logs list.
