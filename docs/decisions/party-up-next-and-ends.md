Decision: A watch party keeps what it plays next and when it starts (migration 0202), set or cleared by its host's player alone (`PUT /api/v1/watch-parties/{id}/up-next`, a file the host's account may watch) and cleared when the party moves to another file; both views show it as `up_next {file_id, title, starts_at}`, or null. Why a guest is no longer in the party is noted (`left`, `sent_away`, or `ended` when the party ends with them in it), kept an hour, and a guest's event stream sends it as a `watch_party_over` event `{reason}` just before its final `null`.

Why: Only the host starts the next episode for everyone, so the others need the host's countdown to show it, read only, rather than counting down on their own and moving alone. A guest whose stream ended couldn't tell being sent away from the party ending or their own leaving, so the player could only say something went wrong. A separate event before `null` keeps players that only know `null` working.

Date: 2026-11-28
