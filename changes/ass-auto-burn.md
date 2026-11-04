### Added

- A player can say which subtitle track the viewer chose (`subtitle`) and whether it draws
  ASS/SSA itself (`renders_ass`) when it asks for a conversion. When it can't and the track is
  styled (signs, positioning, karaoke), Melete burns the track into the picture, as
  `burn_subtitle` does, instead of leaving the player its WebVTT version; a request to copy the
  video keeps copying and says so. The answer gives `burned_subtitle` and `burn_reason`. A
  player that doesn't send `renders_ass` gets what it got before.
