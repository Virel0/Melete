### Added

- A torrent that Deluge says is downloading but that hasn't moved for 4 hours is replaced: its
  title (or album) is searched for again, passing over its release, and when another release
  is grabbed the stalled one is removed from Deluge with its data and its release blocklisted.
  With nothing else to grab it keeps going, says so, and is looked at again 4 hours later.
  Paused and queued torrents aren't stalled.
