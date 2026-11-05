Decision: Melete can download each title's trailer once, through an optional downloader in its
own container (yt-dlp with its JavaScript runtime, and jellyfin-ffmpeg) that shares the VPN
container's network and so can't start or reach YouTube without it. The trailer is picked from
TMDB's video list (YouTube trailers before teasers, official first, in the title's own language,
then the metadata language, then English, then the largest and newest), at most the quality set
(4K, 1080p or 720p; 4K by default), converted once on the GPU to HEVC unless it's already H.264
or HEVC, HDR kept, and kept in the config folder for as long as the title is in the library,
with no size cap. A title with a trailer in its folder gets none, and loses a downloaded one. A
newer trailer on TMDB, or a different quality set, downloads it again, the old one playing
until then; YouTube not having it is kept until another trailer is listed; a failure is tried
again a day later, three times at most. Off by default.

Why: Most titles have no trailer in their folder, and streaming one from YouTube would show
each viewer's address to YouTube. Downloading once through the VPN shows YouTube only the VPN's
address and the video ids; converting once means any TV plays it without converting at each
play. A trailer belongs with its title, so it lives and goes with it.

Date: 2026-11-04
