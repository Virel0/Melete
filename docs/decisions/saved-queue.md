Decision: The web player keeps its queue in the browser under the signed-in user's id, restores
it paused, and forgets every kept queue on signing out. It publishes the queue as its session's
`queue_state` row, in the apps' own shape, through `PUT /music/queue`, which writes only that
session's row in that user's log, for a user who may see all the music; never while listening
privately. Listen home offers another device's queue from the last day.

Why: A reload or a new sign-in lost the queue, and the apps couldn't pick up from the web.
Keying by user and clearing on sign-out keeps one person's listening from showing to the next
person on a shared browser; private listening keeps the queue to the browser too.

Date: 2026-11-07
