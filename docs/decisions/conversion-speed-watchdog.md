Decision: A speed watchdog judges each encoding run once it has worked 30 seconds, paused time aside: below 1.3 times real time it steps down one rung (2160, 1440, 1080, 720, 480 lines; the bit rate no more than that height's default) from the next segment it hasn't made, keeps that for the session through seeks, steps again on another slow spell, and logs each step. A run in fragmented MP4 (HEVC) or with a picture burned in isn't stepped down, only logged once, since its session shares one init section and its burned picture was sized for it.

Why: A conversion that can't keep ahead of the viewer stalls them sooner or later, and a smaller picture is quicker to make; 1.3 times leaves room for a viewer who catches up. A changed picture size in fragmented MP4 needs a new init section mid-playlist, which the playlist doesn't give.

Date: 2026-11-28
