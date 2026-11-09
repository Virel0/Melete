### Fixed

- A missing episode numbered from the first episode ("Some Anime - 31") is grabbed when other
  episodes of its season are on disk or on their way. Before, the grab took it for a pack of the
  whole season and passed it over without saying why. One that can't be matched to an episode
  isn't grabbed.
- An upgrade numbered from the first episode replaces only its own episode's file, which goes to
  the trash once the upgrade is in, as other upgrades' files do.
- Search all missing and cutoff runs say why a result was passed over for what's on disk or on
  its way ("Title, season 2 episode 6: not grabbed: it's on disk already, and this release
  wouldn't replace exactly the files there").
