### Added

- When a Soulseek download of a wanted album or song fails once slskd has it (the peer refused
  a file, it timed out, errored or was cancelled, or slskd lost it), the next best accepted
  Soulseek result of the same search is grabbed, from a user the album or song hasn't failed
  with, up to 3 downloads from a search. The failed download says the next source is being
  tried (2 of 3), and the new one says why while it downloads; when the tries are used up or
  no other user's copy is left, the failed one says so.
