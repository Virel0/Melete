Decision: An ASS/SSA file beside the video is sent in UTF-8 with `charset=utf-8`. One in another
encoding is converted from the encoding guessed for the WebVTT version (its byte-order mark, else
its bytes and the language in its name), once, and cached under the file's name, size and date;
one already in UTF-8 is sent byte for byte, less a byte-order mark. Only the text encoding
changes. When the guess doesn't fit the bytes (decoding finds malformed ones), the file is sent
byte for byte with the guess as its charset, as before.

Why: Players that draw ASS with a libass built without iconv read only UTF-8, and older fansub
files are often in a legacy encoding; the server already knows how to read them.

Date: 2026-11-02
