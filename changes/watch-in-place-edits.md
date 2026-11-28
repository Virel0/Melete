### Fixed

- With the library folders watched, a file edited in place (tags written over by an editor
  that keeps the file open, an edit over a network share whose server doesn't close the file
  at once, or only its times changed) asks for a scan as a new file does, rather than waiting
  for the next scheduled scan. A file still being copied in waits for its copy to finish, as
  before.
