### Changed

- An ASS/SSA file beside the video comes back from its `ass_url` in UTF-8, so players whose
  libass can't convert text encodings can read it. A file in another encoding (Windows-1252,
  Shift_JIS…) is converted once, read the way the WebVTT version reads it, and cached until the
  file changes; a UTF-8 file is still sent byte for byte, less a byte-order mark. Styles and
  override tags are untouched. A file whose encoding can't be told for sure is sent as before.
