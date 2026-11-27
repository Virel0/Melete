### Added

- Certificates under Outside services: Melete gets the certificate for its own HTTPS from
  Let's Encrypt and renews it, off by default. The DNS challenge through deSEC gets a wildcard
  (`*.example.org` with the bare name), so Certificate Transparency logs list no other name.
  The staging server is used until switched off; the account's key is kept sealed, and its URI
  is shown with CAA records that let only it have certificates issued. Renewing follows Let's
  Encrypt's renewal information, else two-thirds through the certificate's life, with retries;
  a renewal that fails alerts admins, saying when the certificate runs out.
  `compose.tls-acme.yaml` serves it (docs/REMOTE-ACCESS.md, section 8).
