Decision: Compressing a title queues its video files that are H.264, MPEG-2, VC-1 or MPEG-4,
leaving out Dolby Vision, which the encoder can't keep. One at a time, and only while nothing is
being converted for anyone, each is converted into the transcode folder: the main video to HEVC
(NVENC at constant quality 22 when the GPU works, else libx265 at CRF 20), 10-bit kept, every
audio, subtitle and font stream copied, as Matroska. The copy replaces the file only if it plays
as long (within half a percent or two seconds), has the same audio and subtitle tracks, is HEVC,
is at least a tenth smaller, and the file is still as scanned: the file moves to the library's
trash (recorded as replaced by the copy, put back like any other), the copy is placed where it
was as `.mkv`, never over anything, and the file's row points at it, so progress stays.

Why: Old H.264 and older copies are often twice the size HEVC needs for the same picture.
Checking length, tracks and saving before anything in the library changes keeps a bad encode
from replacing a good file, and the trash keeps the original for a month.

Date: 2026-11-04
