Decision: File work on library files that has to be recorded (a delete from the trash, a move into or out of it) runs first, on a thread of its own, and only then the short database write that records it. It is waited on for a time limit: 2 minutes for a trash move or delete, an hour for putting a compressed copy in place. Past the limit its library counts as not responding, the caller moves on, and the work is recorded whenever it comes back. Each delete from the trash is logged with its path in its library, size, time taken and result, and the 30-day clean-up deletes the same way as emptying it now.

Why: A share that stopped answering left a delete from the trash waiting for hours: the page hung, nothing was logged, and work that waited behind it stalled. A time limit alone would lose track of a move or delete that finishes later, so the record waits for the work rather than the caller.

Date: 2026-11-27
