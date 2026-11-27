### Fixed

- A watch party's play, pause and seek reach every device and guest at once, rather than at
  their event stream's next look up to two seconds later, so a play set to start a moment
  ahead now starts everyone together.
- A player that hears of a play after its start (it was slow to load, say) starts where the
  party is, instead of from where it was paused and catching up slowly for half a minute.
- When a party pauses, every player stops where it paused, so it plays on together.
