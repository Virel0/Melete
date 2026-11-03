### Added

- **ASS/SSA subtitles as they are, with the video's fonts**, for players that draw ASS
  themselves (positions, signs, fonts, fades and karaoke as authored). The subtitle list marks
  each ASS/SSA track with `ass` and gives its `ass_url` (`subtitles/{track}.ass`, beside the
  stream URL as the WebVTT one is): an embedded track is copied out of the video unchanged once
  and cached, a file beside the video is sent byte for byte. `GET /api/v1/stream/{token}/fonts`
  lists the fonts the video carries (Matroska attachments: name, type, size and URL) and
  `GET /api/v1/stream/{token}/fonts/{font}` serves each, both under the stream link's token.
  The WebVTT version is unchanged.
