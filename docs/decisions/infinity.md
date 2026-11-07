Decision: Infinity on the web is off until switched on, and the browser keeps the choice. When
fewer than three songs are left after the one playing, it asks `POST /aoide/next` once, seeded
by that song, with what's queued and what the tab played lately, and puts the answer at the end
of the queue in the server's order, leaving out songs already queued. It asks once for each song
and queue length, so an answer with nothing new isn't asked again until the queue moves on. It
rests while the queue repeats, during a Listen Together session, and for someone who may see
only some music libraries. Private listening doesn't stop it. A queue that had already run out
plays on into the songs it adds.

Why: The Aoide apps' rules, so the queue keeps going the same way on every device. A repeating
queue never runs out, and in a session every device would add songs of its own. Asking what
comes next records no listen and keeps nothing on the server, which is all Private listening
promises. Songs added to a queue that ran out are what the listener switched Infinity on for.

Date: 2026-11-07
