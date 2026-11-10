//! MusicBrainz's settings: matching music, the covers of music not in the library that a search
//! for a request may show, artists' portraits, and For You from ListenBrainz, which go by the
//! MusicBrainz ids matching gives.

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
            "In searches for music to request and for the library's artists, from Wikimedia Commons, with their author and licence. The artist's MusicBrainz id goes to MusicBrainz for its Wikidata link, that item's id to wikidata.org for its image, and the image's file name to commons.wikimedia.org and its picture hosts (upload.wikimedia.org, thumb.wikimedia.org), which see the server's address. The library's artists are looked up in the background, 20 an hour; one with artist.jpg or folder.jpg in the folder above their albums has that picture instead and isn't looked up. The portrait is fetched once and cached: those a search showed for 30 days, 2000 at most, and the library's artists while they're in it. Needs MusicBrainz matching on.",
            Input::Toggle { on: "on" },
        ),
        field(
            "MELETE_LISTENBRAINZ",
            "For You from ListenBrainz",
            "Songs for each person from what they've played, some in the library to play and some not, to request, from ListenBrainz's open listening data. Only MusicBrainz ids leave the server: those of the recordings and artists each person played most in the last 90 days go to labs.api.listenbrainz.org, for the recordings and artists listened to alongside them; those of the closest artists alongside go to api.listenbrainz.org, for their most listened-to recordings, and so do those of the recordings suggested, for their artists and album. Never a name, an account, play counts, titles or anything else about the household; no ListenBrainz account is used. Each person's list is made in the background, again at most once a day while what they play changes, a few people an hour at one request a second; answers are kept a week. Covers of songs to request need covers of music not in the library on. Turned off, every list and kept answer is forgotten. Needs MusicBrainz matching on.",
            Input::Toggle { on: "on" },
        ),
    ],
};
