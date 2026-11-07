### Changed

- Someone a smart playlist is shared with to edit can change its rules on the web too (Edit
  rules, `PUT /playlists/{id}/rules`), as their devices may; the change goes into their own sync
  log and reaches the owner's devices. One shared to listen to still can't.
