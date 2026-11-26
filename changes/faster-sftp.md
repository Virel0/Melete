### Changed

- Copies home from the seedbox keep up to 32 MiB in flight instead of 2 MiB: one copy moves
  about ten times as much per round trip, so a far seedbox that held copies to a few MB/s
  no longer does.
