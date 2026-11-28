### Fixed

- A copied stream started partway through a file (after a seek, or joining a watch party late)
  no longer starts its first segment with a fraction of a second of picture and no sound, which
  players stalled at.
