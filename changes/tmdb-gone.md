### Fixed

- A movie or series TMDB no longer has is asked about once a day, not at every run, so "TMDB no
  longer has this series" is logged once a day rather than every hour. Its cached details are
  still shown, and it's listed under Needs a match (status `gone` in
  `GET /api/v1/metadata/unmatched`, and in the titles-to-match note on the health page) until
  TMDB has it again or it's matched to another entry.
