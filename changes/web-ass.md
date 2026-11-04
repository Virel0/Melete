### Added

- The web player draws styled ASS/SSA subtitles as they were made, with their signs, placement,
  fonts and karaoke, over the video, instead of their WebVTT version. It uses JASSUB (libass in
  WebAssembly), served by Melete with the fonts the video carries; nothing is fetched from
  anywhere else. Where a browser can't draw them, the WebVTT version shows, as before.
