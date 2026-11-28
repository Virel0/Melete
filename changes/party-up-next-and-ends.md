### Added

- A watch party can say what it plays next and when: the host's player sets it as it counts
  down to the next episode (`PUT /api/v1/watch-parties/{id}/up-next`), and everyone's player,
  guests' too, sees the same `up_next`. Only the host sets it; it's cleared when the party
  moves on.
- A guest's event stream says why it ends before it does: a `watch_party_over` event with the
  reason (`ended`, `sent_away` or `left`), then `null` as before.
