Decision: The seedbox's SSH channel offers a 32 MiB window (russh's default is 2 MiB), and SFTP keeps enough reads asked for at once to fill it (129 of 255 KiB). One connection still copies one file at a time; files aren't split across connections.

Why: An SFTP copy moves at most its window per round trip, so 2 MiB held one copy to about 10 MB/s at 100 ms and 5 MB/s at 200 ms. Measured against OpenSSH through a delay of 200 ms, a 32 MiB window moved 49 MB/s on one connection. Several connections per file help only when the seedbox limits each connection; with the window large enough that's not needed to fill a link, and one connection keeps the copy simple to resume.

Date: 2026-11-26
