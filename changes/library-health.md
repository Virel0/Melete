### Added

- Each library's folder is read every minute, on a thread of its own and with a 15-second
  limit. A library whose folder doesn't answer in time, can't be read or is empty (an
  unmounted share looks empty) is marked as not responding until it answers again: System
  shows it as a problem (`library_unreachable`), scans leave it alone, and playing one of its
  files says "the movies library isn't responding" (503) instead of waiting. A restart keeps
  what was known until the next read.

### Fixed

- A frozen library share no longer ties up Melete: streams, artwork from library folders,
  the System page's disk space and admin health read library folders off the async threads
  with a time limit, and a scan that was walking a library gives up once it stops answering,
  so other libraries are still scanned.
