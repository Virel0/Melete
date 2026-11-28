Decision: The library watch (`MELETE_SCAN_WATCH`) also counts a file written to in place (`IN_MODIFY`) and a file whose times or permissions changed (`IN_ATTRIB`), besides finished writes, moves, deletions and new folders. A file made since the watch started is still being written, so its writes and new times count only once its write finishes (`IN_CLOSE_WRITE`); writes to any other file count at once. A folder's own changes of this kind don't count. As before, the scan asked for waits until the library has been quiet for 30 seconds.

Why: Finished writes alone missed edits whose file isn't closed soon after the write: a tag editor that keeps the file open, and a network share's server, which may hold the file open after its client is done. An editor that sets the times back after writing (as some tag editors offer) changes only the times. Holding back a new file's writes keeps a copy in progress from being scanned half-done.

Date: 2026-11-28
