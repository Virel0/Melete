Decision: Certificates can also prove the name by TLS-ALPN-01: the domain is then the exact name Melete is reached at, HTTPS (which must be set to listen) offers acme-tls/1 after http/1.1, and a handshake offering acme-tls/1 and nothing else gets the challenge certificate for the name asked, only while an order for it waits, else no certificate at all; no HTTP is served on such a connection, and any other client gets HTTPS's own certificate. It works after a trusted proxy's PROXY header, as every connection does.

Why: A setup passing HTTPS through a VPS can then get its certificate with no DNS token on the server and nothing at deSEC, at the cost of the exact name in Certificate Transparency logs, which DNS-01's wildcard avoids. Serving the challenge certificate to a client offering acme-tls/1 alone, as RFC 8737 has the CA do, means no ordinary client ever sees it, and none is served when nothing waits.

Date: 2026-11-27
