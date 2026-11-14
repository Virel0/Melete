### Fixed

- Search on a title's page takes a season found only as single episodes: when no pack of it is
  accepted, its single episodes are grabbed for the season's missing episodes, as an approved
  request or Search all missing already did (`POST /api/v1/searches/{id}/take`). Before, it
  said nothing was accepted and grabbed nothing, though its results said they'd be taken.
