Decision: ASS/SSA tracks are also served as they are, beside their WebVTT version, with the
fonts the video carries, all under the stream link's token. An embedded track is copied out with
ffmpeg without re-encoding and cached apart from the burn-in rendering; a file beside the video
is sent byte for byte with its encoding as the charset; 20 MB at most. Fonts are only the
attachments that are fonts (by stored type or name), 64 MB each at most, named in URLs by their
stream index and looked up in the video's own list, so no name a client sends becomes a path.
Their names lose any folders, and their types come from a fixed set of font types, never the
stored type, so nothing in a video decides what a browser takes a response for.

Why: WebVTT keeps the words and timing but drops styling, positions and fonts, which styled
anime tracks need to be readable as made, and burning them in forces a conversion. Players that
link libass can draw them, and only need the original track and its fonts from the server.

Date: 2026-11-02
