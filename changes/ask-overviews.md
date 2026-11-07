### Added

- Ask for something's movie and series results show each title's summary under its name,
  clipped to three lines, and so do the search page's titles not in the library. `GET
  /requests/search` carries it as `overview`, from the same TMDB search answer that found the
  title, so nothing more is asked of TMDB.
