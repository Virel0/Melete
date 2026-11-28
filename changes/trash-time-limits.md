### Changed

- Each file deleted from the trash, now or after its 30 days, is logged with its path in its
  library, its size, how long the delete took and how it went.

### Fixed

- A delete from the trash, a move into it (an upgrade's old file, a wrong file, a removed
  title, a compressed file's original) or a move back out that doesn't finish within its time
  limit no longer holds up the page, the next file or the background work after it. Its
  library counts as not responding, the files after it in that library are left at once, and
  what was done is recorded once the share answers.
- Probing and measuring a file, and reading a compressed copy's size, no longer wait without
  end on a library that stopped answering.
