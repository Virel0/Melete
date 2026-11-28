### Changed

- Empty the trash now runs in the background: the trash page shows how many files are deleted,
  how many were left and how much space is freed while it runs, and stays usable meanwhile.
  `POST /api/v1/trash/empty` starts it and `GET /api/v1/trash/emptying` says how far it has got;
  `DELETE /api/v1/trash` still empties it and answers when done.
