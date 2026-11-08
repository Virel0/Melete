### Added

- An offline copy's link carries its seek-bar previews, so an app can keep them with the
  download: `trickplay` on `POST /offline/{id}/link` gives the preview index and a URL for each
  sheet, served under the link's own token (`/offline/download/{token}/trickplay/{n}.jpg`), as
  its subtitles are. Null when the file has none, or has changed since the copy was made.
