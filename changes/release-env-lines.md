### Changed

- Release notes give every image (Melete, the stem separator, the trailer downloader) as a line
  ready to paste into `.env`, with its full digest under the variable its compose file reads:
  `MELETE_IMAGE=ghcr.io/…:<version>@sha256:…`, `MELETE_STEMS_IMAGE=…`, `MELETE_TRAILERS_IMAGE=…`.
