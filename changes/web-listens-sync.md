### Changed

- A listen from the web UI (or any app that sends listens through the music API) is also
  written into the listener's sync log as an Aoide play event, so it counts on their Aoide
  devices: Replay, finish rates, Top songs and play-count rules. Reading the log doesn't keep it
  a second time. Listens from before this release aren't copied.
- A song flagged "Don't count plays" (on an Aoide device) has no new listens kept from the web,
  as the apps already do; `POST /music/plays` answers 409.
