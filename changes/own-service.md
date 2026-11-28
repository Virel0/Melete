### Added

- Each of Melete's images checks at start which compose service it runs as (`MELETE_SERVICE`,
  set by each compose file), and an image put on another service's variable stops at once with
  a message naming the variable to check, such as "this is the trailer downloader's image, but
  it's running as Melete: check MELETE_IMAGE", instead of crash-looping on an error that
  doesn't point at the cause.
