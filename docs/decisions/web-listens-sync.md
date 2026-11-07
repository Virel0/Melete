Decision: A listen kept through `POST /music/plays` is also appended to the listener's sync log
as the play event an Aoide device would push (every field the apps require, the session as the
device, the listen's id as the event's), so it counts on their devices; reading the log passes
over a play event whose id the history already has from the API. A track flagged "don't count
plays" has no new listens kept (409). Deleting a listen from Melete's history writes nothing to
the log, since the apps treat play events as append-only and quarantine deletes.

Why: Browser listens counted on Melete's Home and mixes but on no device, so a play anywhere
didn't count everywhere; one stored copy plus one log copy keeps the history from counting it
twice.

Date: 2026-11-06
