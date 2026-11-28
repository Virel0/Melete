Decision: Each compose service of Melete's own (melete, stems, trailers) sets `MELETE_SERVICE` to its name, and each of Melete's images checks it before anything else starts: another of Melete's services, or a name it doesn't know, stops it with a message saying which image it is, what it runs as, and the `.env` variable to check (`MELETE_IMAGE`, `MELETE_STEMS_IMAGE`, `MELETE_TRAILERS_IMAGE`). Unset, as when an image is run outside compose, nothing is checked.

Why: The three images live in one package with tags and digests that look alike, so a digest pasted on the wrong variable is an easy slip, and the program it starts then fails on something unrelated (a read-only file system), crash-looping. Compose knows which service it starts, so it says so, and the image needs no guessing from other settings.

Date: 2026-11-28
