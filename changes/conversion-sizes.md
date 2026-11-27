### Changed

- A conversion held to a bitrate (a way in's limit, a watch party's cap for guests, a guest's or
  a player's chosen quality) is made smaller to suit it: up to 4 Mbit/s 720p, up to 10 Mbit/s
  1080p, up to 20 Mbit/s 1440p. Above that H.264 stays at 1440p and HEVC keeps the source's
  size. A 4K source held to an 8 Mbit/s way now comes as 1080p rather than 4K at a bitrate too
  low for it.
- HDR is tone mapped after the picture is scaled down, on the GPU and the CPU alike, so tone
  mapping works on the smaller picture.
- While a conversion runs for someone watching, seek-bar previews, intro and credits detection
  and trailers' sound levelling wait before starting their next file, as Make room's
  compressing already did.
- Each video conversion says in the log when it starts (why, from what to what, on the GPU or
  the CPU) and when it stops (how quickly it went against real time, and why it stopped).

### Fixed

- A watch party guest plays a file within the party's cap as it is, or with only its container
  and audio converted, as a household player does, instead of always having it converted.
- A guest's copied video in a party with a cap no longer loses its playlist link at once.
