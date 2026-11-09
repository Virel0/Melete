//! MusicBrainz's settings: matching music, and the covers of music not in the library that a
//! search for a request may show.

use super::*;

pub(super) const MUSICBRAINZ: Service = Service {
    key: "musicbrainz",
    name: "MusicBrainz",
    about: "Matching music, album covers, and looking up music to request. Album and track names leave the server.",
    fields: &[
        field(
            "MELETE_MUSICBRAINZ",
            "Match music to MusicBrainz",
            "One request a second; no account needed.",
            Input::Toggle { on: "on" },
        ),
        field(
            "MELETE_SEARCH_COVERS",
            "Show covers of music not in the library",
            "In searches for music to request. Fetched once by the server from the Cover Art Archive (coverartarchive.org and archive.org), which sees the release group's id and the server's address, and cached: only those a search showed, for 30 days, 2000 at most. Needs MusicBrainz matching on.",
            Input::Toggle { on: "on" },
        ),
        field(
            "MELETE_ARTIST_PORTRAITS",
            "Show artists' portraits",
            "In searches for music to request, from Wikimedia Commons, with their author and licence. The artist's id goes to MusicBrainz for its Wikidata link, that item's id to wikidata.org for its image, and the image's file name to commons.wikimedia.org and its picture hosts (upload.wikimedia.org, thumb.wikimedia.org), which see the server's address; the portrait is fetched once and cached, only those a search showed, for 30 days, 2000 at most. Needs MusicBrainz matching on.",
            Input::Toggle { on: "on" },
        ),
    ],
};
