Decision: The web UI listens together through the same session routes and `listening` event as
the apps, and offers only what the server allows: Start and Join to anyone who can see every
music library, Leave to everyone in a session, End for everyone only to whoever started it or an
admin. A change made on another device first wins: a refused change is dropped and the newer
state followed. Pressing play on the session's track catches up with the session rather than
sending this device's place, and a device more than 1.5 seconds off seeks to it.

Why: The server already keeps one version-checked state per session, so the web needs no new
route; showing only the actions the server would accept keeps refusals out of the way, and
catching up rather than sending on play keeps a device that was paused or reloaded from pulling
everyone back.

Date: 2026-11-07
