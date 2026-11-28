Decision: A run that copies the video and starts partway through the file seeks with `-noaccurate_seek`, so the converted audio isn't cut at the seek point but starts where the file's audio does around the keyframe the video starts on, as it does in a run from the start of the file.

Why: The seek point is a little past the keyframe (so ffmpeg lands on it, more so with B-frames), and accurate seeking cut the decoded audio there while the copied video kept the keyframe: each mid-file run's first segment had up to a few tenths of a second of picture with no sound, and players stalled at that hole.

Date: 2026-11-28
