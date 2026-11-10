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
        field(
            "MELETE_SONG_YOUTUBE",
            "Songs from YouTube as a last resort",
            "A song someone asked for that Soulseek found nothing for, or whose Soulseek downloads all failed, is looked for on YouTube by its artist and title, through the VPN. A video is taken only when its title, artist and length all fit the song, and never a cover, remix or sped-up version. YouTube's audio is lossy (Opus or AAC, about 130 to 160 kbps) and is kept as it comes, whatever the music profile prefers.",
            Input::Toggle { on: "on" },
        ),
    ],
};
