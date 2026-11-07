Decision: The web sets "not interested" and "don't count plays" by writing into the listener's
sync log the `track_flags` row an Aoide device would: the track's own row (its latest op) when it
has one, else a new one, with only the changed flag stamped in `fieldUpdatedAt` (a flag the row
never stamped keeps the row's old time), so a change to the other flag on a device isn't undone.
Like the rest of the log, flags are closed to a listener limited to some music libraries.

Why: The apps already keep these flags in the log and merge them flag by flag; writing the same
row keeps one place for them and lets every device follow a change made in the browser.

Date: 2026-11-06
