### Fixed

- A pack of several seasons ("S01-S07") grabbed for one season brings home every monitored
  season it holds missing episodes of, one season after another, each into its own folder and
  checked against its own episodes, instead of only the season it was grabbed for. While it
  downloads those seasons count as on their way, so their searches don't grab another pack.
  Packs brought home for one season before, whose files are still on the seedbox, are opened
  again for the seasons still missing.

### Changed

- Of releases as good as each other, a season's own pack is grabbed before a pack of several.
