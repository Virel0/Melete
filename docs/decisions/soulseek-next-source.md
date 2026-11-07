Decision: A Soulseek download of a wanted album or song that fails once slskd has it (a file
rejected, timed out, errored or cancelled by the peer, or a batch slskd lost) is followed by the
next best accepted Soulseek result of the search it came from, in the order grabbing ranks them,
from a user none of the album's or song's downloads has failed with. A search gets 3 Soulseek
downloads at most, the first included, and a queue slskd refuses counts as one. Nothing is
grabbed when the album or song is on disk or on its way meanwhile, or isn't wanted or monitored
any more. Downloads record their search (migration 0135) to count this. The failed download says
the next source is being tried (2 of 3), or that the tries are used up or no other user's copy
is left; the next one keeps why it's from there before slskd's word while it downloads. A
download whose result was its search's only accepted Soulseek one keeps slskd's word alone.
Torrents keep their own handling.

Why: A Soulseek user often lists a file and then refuses to send it, and the request stopped
there although the same search had found it elsewhere. Another user is asked rather than another
folder of the same one, since the same peer tends to refuse again; 3 tries keep a request from
working through a whole search of peers who won't send.

Date: 2026-11-06
