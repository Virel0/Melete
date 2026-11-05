//! Trailer downloads' settings: on or off, and the most a trailer is downloaded at.

use super::*;

pub(super) const TRAILERS: Service = Service {
    key: "trailers",
    name: "Trailer downloads",
    about: "Each movie's, series' and anime's trailer, downloaded once from YouTube and kept while the title is in the library, so the hero and title pages play it like a trailer in its folder (which always wins). Which trailer is picked from TMDB's list of the title's videos; only those video ids go to YouTube, and only through the VPN the trailer downloader runs behind.",
    fields: &[
        field(
            "MELETE_TRAILER_DOWNLOADS",
            "Download trailers",
            "Needs the trailer downloader beside Melete, behind the VPN. Each is converted once on the GPU, so any TV plays it.",
            Input::Toggle { on: "on" },
        ),
        field(
            "MELETE_TRAILER_QUALITY",
            "Quality",
            "The most a trailer is downloaded at. Changing it downloads each trailer again, the old one playing until then.",
            Input::Choice {
                options: &[
                    Choice::new("2160", "4K"),
                    Choice::new("1080", "1080p"),
                    Choice::new("720", "720p"),
                ],
            },
        ),
    ],
};
