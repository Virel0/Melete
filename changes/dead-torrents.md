### Added

- A torrent no one is sharing is found dead and said so: when Deluge has seen no seeder of a
  download (none connected, none counted by its trackers, no whole copy among its peers) and
  it has made no progress for 24 hours, and another search finds nothing else to grab, it's
  removed from Deluge with what it had on the seedbox, its release is blocklisted, and its
  download says no one is sharing it. A title's progress and each season's carry `dead`, so
  the Requests page and the apps tell whoever asked that nothing will come of what was found;
  a title still looked for stays searching, in case another release appears.
