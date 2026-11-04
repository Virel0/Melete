Decision: A conversion request may name the subtitle track chosen (`subtitle`) and say whether
the player draws ASS/SSA (`renders_ass`). With `renders_ass` false and a styled ASS/SSA track
chosen, the track is burned in as `burn_subtitle` would; styled means a dialogue line uses a
typesetting tag (positioning, movement, clipping, rotation, drawing, animation or karaoke) or
the dialogue uses more than one style. Whether a track is styled is decided once per version of
the track. A copy request keeps copying (`burn_reason` copy_kept). Without `renders_ass`, or
with it true, nothing is burned in unasked.

Why: WebVTT keeps only the words, so a styled track lost its signs and placement on players that
can't draw ASS; burning in only styled tracks, and only when the player says it can't draw them,
never forces a conversion for plain dialogue or on players that can.

Date: 2026-11-04
