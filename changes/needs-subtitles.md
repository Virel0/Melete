### Added

- A quality profile can need subtitles in a language (Needs subtitles in, `subtitles_language`):
  unless a download's audio is in that language, each video must have subtitles in it, inside
  the file or beside it. A raw (no subtitles), or one with subtitles in other languages only, or
  only signs and songs, is set aside at import, blocklisted and searched again. Anime profiles
  start with English, existing ones included; others ask nothing.
