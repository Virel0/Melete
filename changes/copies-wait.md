### Changed

- A copy home into a library that isn't responding isn't started: the download waits, says
  "waiting: the movies library isn't responding", and is looked at again each minute without
  using up its tries. A copy already running stops and waits the same way once its library
  stops responding, or once a write into it takes over a minute.
- A copy that copies nothing for 10 minutes is marked as stalled ("stalled: nothing was
  copied for 10 minutes") and starts again on its own, from where it stopped.

### Fixed

- A copy's file steps (checking the library is mounted, making folders, opening and measuring
  part files, putting files in place) run off the async threads with a time limit, so a
  frozen share can't hold up every other transfer queued behind it.
