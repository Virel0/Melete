//! Every outside service the web UI can set up, with its settings and how each is entered.

use super::*;

/// How a setting is entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Input {
    Text,
    /// Written once, never shown again.
    Secret,
    Number,
    /// On or off; `on` is the value that means on (lyrics' is "lrclib").
    Toggle {
        on: &'static str,
    },
    /// One of a list; the first is the default.
    Choice {
        options: &'static [Choice],
    },
}

/// One value a setting picked from a list may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Choice {
    /// What's stored.
    pub value: &'static str,
    /// What the web UI calls it.
    pub label: &'static str,
}

impl Choice {
    /// A value and what it's called.
    pub const fn new(value: &'static str, label: &'static str) -> Choice {
        Choice { value, label }
    }
}

/// One setting of an outside service, as the web UI shows it.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Field {
    /// The environment variable it stands for.
    pub name: &'static str,
    /// What the web UI calls it.
    pub label: &'static str,
    /// A line on what to enter.
    pub hint: &'static str,
    /// How it's entered.
    pub input: Input,
}

/// One outside service and its settings.
#[derive(Debug, Clone, Copy)]
pub struct Service {
    /// The service's id in the API, e.g. "tmdb".
    pub key: &'static str,
    /// What the web UI calls it.
    pub name: &'static str,
    /// What it's for, and what leaves the server when it's on.
    pub about: &'static str,
    /// Its settings, in the order they're shown.
    pub fields: &'static [Field],
}

pub(super) const fn field(
    name: &'static str,
    label: &'static str,
    hint: &'static str,
    input: Input,
) -> Field {
    Field {
        name,
        label,
        hint,
        input,
    }
}

/// What can be set up from the web UI. Paths, libraries, the address Melete listens on and the
/// ways in stay in the environment: they're the container's, and a wrong one from the UI could
/// lock everyone out.
pub const SERVICES: &[Service] = &[
    Service {
        key: "tmdb",
        name: "TMDB",
        about: "Titles, posters, episode lists and content ratings for movies and series. Titles to look up leave the server, and the language chosen for them.",
        fields: &[
            field(
                "MELETE_TMDB_TOKEN",
                "API Read Access Token",
                "From your TMDB account's API settings (the long token, not the API key).",
                Input::Secret,
            ),
            field(
                "MELETE_METADATA_LANGUAGE",
                "Language",
                "Titles, summaries, episode names and logos in this language, such as fr-FR or es-ES; English (en-US) when empty. What TMDB hasn't translated comes in English. Changing it fetches every title again at the next metadata run.",
                Input::Text,
            ),
            field(
                crate::ratings::COUNTRY_SETTING,
                "Age ratings from",
                "Users with a highest rating see a title by this country's rating for it, the United States' when it has none, and don't see it with neither. A change counts at once, with nothing fetched again.",
                Input::Choice {
                    options: crate::ratings::COUNTRIES,
                },
            ),
        ],
    },
    Service {
        key: "musicbrainz",
        name: "MusicBrainz",
        about: "Matching music, album covers, and looking up music to request. Album and track names leave the server.",
        fields: &[field(
            "MELETE_MUSICBRAINZ",
            "Match music to MusicBrainz",
            "One request a second; no account needed.",
            Input::Toggle { on: "on" },
        )],
    },
    Service {
        key: "lyrics",
        name: "Lyrics (LRCLIB)",
        about: "Synced lyrics for imported music, and lyrics for the apps of tracks that have none of their own. The title, artist, album and length of each track looked up leave the server, once per track.",
        fields: &[field(
            "MELETE_LYRICS",
            "Fetch synced lyrics",
            "From lrclib.net: as .lrc files beside imported tracks, and kept in Melete for the rest.",
            Input::Toggle { on: "lrclib" },
        )],
    },
    Service {
        key: "translation",
        name: "Lyrics translation (LibreTranslate)",
        about: "Translating lyrics into the language an app asks for. The lines of each song translated leave the server, to the LibreTranslate set here; run one yourself to keep them in the household.",
        fields: &[
            field(
                "MELETE_LIBRETRANSLATE_URL",
                "LibreTranslate's address",
                "As Melete reaches it, e.g. http://libretranslate:5000.",
                Input::Text,
            ),
            field(
                "MELETE_LIBRETRANSLATE_API_KEY",
                "API key",
                "Only for a server that wants one.",
                Input::Secret,
            ),
        ],
    },
    Service {
        key: "romanisation",
        name: "Lyrics pronunciation",
        about: "Lyrics in Japanese, Korean or Cyrillic shown with their pronunciation in Latin letters, beside the line and its translation, for apps that ask. Worked out on the server; nothing leaves it.",
        fields: &[field(
            "MELETE_LYRICS_ROMANISATION",
            "Show lyrics' pronunciation",
            "Japanese in Hepburn (kanji read in context with a built-in dictionary), Korean in the Revised Romanization, Cyrillic letter by letter.",
            Input::Toggle { on: "on" },
        )],
    },
    Service {
        key: "slskd",
        name: "Soulseek (slskd)",
        about: "Album and song searches on Soulseek, through slskd running beside Melete. Search words go to the Soulseek network under slskd's account.",
        fields: &[
            field(
                "MELETE_SLSKD_URL",
                "slskd's address",
                "As Melete reaches it, e.g. http://slskd:5030.",
                Input::Text,
            ),
            field(
                "MELETE_SLSKD_API_KEY",
                "API key",
                "One of the keys in slskd's web.authentication.api_keys, with the ReadWrite role and no more.",
                Input::Secret,
            ),
            field(
                "MELETE_SLSKD_DOWNLOADS",
                "Its downloads folder, as Melete sees it",
                "Where slskd's finished downloads are mounted in Melete's container; they stay there until this is set.",
                Input::Text,
            ),
        ],
    },
    Service {
        key: "downloads",
        name: "Deluge",
        about: "Grabbing torrents with Deluge on the seedbox.",
        fields: &[
            field(
                "MELETE_DOWNLOADS",
                "Download torrents",
                "Off: searches still run, nothing is grabbed.",
                Input::Toggle { on: "on" },
            ),
            field(
                "MELETE_DELUGE_URL",
                "Deluge's web UI",
                "As Melete reaches it, e.g. https://seedbox.example/deluge.",
                Input::Text,
            ),
            field(
                "MELETE_DELUGE_PASSWORD",
                "Web UI password",
                "",
                Input::Secret,
            ),
        ],
    },
    Service {
        key: "seedbox",
        name: "Seedbox (SFTP)",
        about: "Copying finished torrents home from the seedbox. Without it, downloads are followed but stay on the seedbox.",
        fields: &[
            field("MELETE_SEEDBOX_HOST", "Host", "", Input::Text),
            field(
                "MELETE_SEEDBOX_PORT",
                "Port",
                "22 unless your provider says otherwise.",
                Input::Number,
            ),
            field("MELETE_SEEDBOX_USER", "User", "", Input::Text),
            field("MELETE_SEEDBOX_PASSWORD", "Password", "", Input::Secret),
            field(
                "MELETE_SEEDBOX_HOST_KEY",
                "Host key",
                "The server's key fingerprint (SHA256:…); `melete seedbox host-key <host> <port>` shows it. Check it with your provider.",
                Input::Text,
            ),
            field(
                "MELETE_SEEDBOX_PATHS",
                "Folders",
                "<Deluge's folder for finished torrents>=<the same folder over SFTP>, e.g. /home/user/Completed=/Completed.",
                Input::Text,
            ),
        ],
    },
    Service {
        key: "flaresolverr",
        name: "FlareSolverr",
        about: "Reaching trackers behind Cloudflare's browser check (Uindex), through FlareSolverr or a solver that copies its API, such as Byparr. The Test checks the solver answers and fetches no page, so whether it gets past a check shows on the first search through it.",
        fields: &[field(
            "MELETE_FLARESOLVERR_URL",
            "FlareSolverr's address",
            "e.g. http://flaresolverr:8191.",
            Input::Text,
        )],
    },
    Service {
        key: "oidc",
        name: "Sign-in through an identity provider (OIDC)",
        about: "Signing in through your own account provider (Authentik, Authelia, Keycloak and the like) with OpenID Connect. Each user links their account under Settings first. Only Melete's client id and secret and the sign-in's code go to the provider, and only an opaque id for the user comes back.",
        fields: &[
            field(
                "MELETE_OIDC_ISSUER",
                "Issuer",
                "Exactly as the provider names itself, e.g. https://auth.example/application/o/melete/. Must be https.",
                Input::Text,
            ),
            field(
                "MELETE_OIDC_CLIENT_ID",
                "Client id",
                "From registering Melete with the provider, as a confidential client.",
                Input::Text,
            ),
            field(
                "MELETE_OIDC_CLIENT_SECRET",
                "Client secret",
                "",
                Input::Secret,
            ),
            field(
                "MELETE_OIDC_REDIRECT_URL",
                "Callback address",
                "Melete's address as browsers reach it, then /api/v1/auth/oidc/callback; register the same with the provider.",
                Input::Text,
            ),
            field(
                "MELETE_OIDC_APP_RETURNS",
                "Where apps return to",
                "The addresses apps signing in through the provider return to, each exactly as the app sends it (e.g. com.example.app:/signed-in), separated by spaces. Empty: apps can't sign in through it, except to a port on the device itself (http://127.0.0.1:… or http://[::1]:…), which is always allowed and needs no listing.",
                Input::Text,
            ),
            field(
                "MELETE_OIDC_NAME",
                "Name on the sign-in button",
                "e.g. Authentik. The issuer's host if left empty.",
                Input::Text,
            ),
        ],
    },
    Service {
        key: "openrouter",
        name: "AI features (OpenRouter)",
        about: "Finding movies and series by describing them, with one key on the server so no device needs its own. What someone types to describe what they're after goes to OpenRouter and the model it picks; nothing about the library or who asked. OpenRouter is asked to use only providers that don't keep or train on it. Each search costs a little on the OpenRouter account; each user may make 30 an hour.",
        fields: &[
            field(
                "MELETE_OPENROUTER_KEY",
                "API key",
                "From your OpenRouter account's Keys page. Set a credit limit on it there.",
                Input::Secret,
            ),
            field(
                "MELETE_OPENROUTER_MODEL",
                "Model",
                "An OpenRouter model id. Empty: openrouter/auto, which picks one for each request.",
                Input::Text,
            ),
        ],
    },
    Service {
        key: "auto_dj",
        name: "Auto DJ",
        about: "Mixing from one track into the next, like a DJ: Melete splits music into its parts (voice, drums, bass, guitar, other) with the stem separator and plans the transitions. Nothing leaves the server.",
        fields: &[field(
            "MELETE_AUTO_DJ",
            "Turn on Auto DJ",
            "Needs the stem separator (compose.stems.yaml) to split music into its parts.",
            Input::Toggle { on: "on" },
        )],
    },
    Service {
        key: "motion_covers",
        name: "Motion covers",
        about: "Looping motion covers made from albums' still covers, for albums without one of their own: a slow, gentle camera move over the cover's depth, with lettering and faces kept whole. Covers that are mostly text or flat are skipped. Each user can hide the ones made here. Made on the server; nothing about the library leaves it.",
        fields: &[field(
            "MELETE_MOTION_COVERS",
            "Make motion covers",
            "Needs the motion cover renderer beside Melete (MELETE_MOTION_JOBS). It waits while video is transcoded or music is split into parts.",
            Input::Toggle { on: "on" },
        )],
    },
    Service {
        key: "discovery",
        name: "Trending and similar titles",
        about: "Rows of what's trending on TMDB and of titles like one being looked at, and the films of a franchise that aren't in the library, each marked playable, requested or available to request. Only which chart (today's or this week's), a title's TMDB id or a collection's TMDB id leaves the server, never anything about the household. Needs TMDB.",
        fields: &[
            field(
                "MELETE_DISCOVERY",
                "Show trending and similar titles",
                "",
                Input::Toggle { on: "on" },
            ),
            field(
                "MELETE_DISCOVERY_POSTERS",
                "Show posters of titles not in the library",
                "Fetched once from TMDB's image server and cached: only those a list showed, at one size, for 30 days, 2000 at most.",
                Input::Toggle { on: "on" },
            ),
        ],
    },
    Service {
        key: "trailers",
        name: "Trailers online",
        about: "A trailer for titles without one in their folder, played through YouTube's embedded player. The server asks TMDB for each library title's list of videos, by the TMDB id it already uses (one more request a title, again whenever its details are refreshed); nothing goes to YouTube from the server. Nothing loads from YouTube until someone presses play: then that viewer's own browser or device loads the trailer from youtube-nocookie.com (YouTube's privacy-enhanced mode, no cookies before playback), so Google sees that device and that one trailer, never the library. Off, no video lists are asked for and no player is offered. Needs TMDB.",
        fields: &[field(
            "MELETE_TRAILERS_ONLINE",
            "Show trailers from YouTube",
            "Trailers in a title's folder still come first.",
            Input::Toggle { on: "on" },
        )],
    },
    Service {
        key: "scans",
        name: "Library scans",
        about: "How often every library is scanned whole, to find files put there by hand or by other tools. A whole scan lists every folder, which wakes sleeping disks; what Melete imports, and a folder an admin rescans, is picked up at once on its own. A folder holding a .ignore or .melete-ignore file is skipped, and what's recorded under it stays. Nothing leaves the server.",
        fields: &[
            field(
                "MELETE_SCAN_SCHEDULE",
                "Scan every library",
                "Every so many hours, such as 6h (the default) or 24h, or once a day at a time on the server's clock, such as 04:00.",
                Input::Text,
            ),
            field(
                "MELETE_SCAN_WATCH",
                "Watch the folders for changes",
                "Scans a title's folder about 30 seconds after something in it changes, without waiting for the schedule. Best effort: network and pooled filesystems often don't report changes, and a large library may need fs.inotify.max_user_watches raised, so the schedule still runs. Turning it on lists every folder once.",
                Input::Toggle { on: "on" },
            ),
        ],
    },
    Service {
        key: "intro_detection",
        name: "Intros and credits",
        about: "Finding intros and credits, so players can offer to skip them: each episode's opening and end are compared with others of its season, and the end of a movie, or of an episode whose credits that didn't find, is looked at alone for a long run of black or silent seconds. Nothing leaves the server.",
        fields: &[field(
            "MELETE_INTRO_DETECTION",
            "Look for intros and credits",
            "The first run reads part of every episode's audio and the end of every movie, which takes a while on a large library.",
            Input::Toggle { on: "on" },
        )],
    },
    Service {
        key: "trickplay",
        name: "Seek-bar previews",
        about: "Small pictures on the seek bar, so you can see where a seek lands: one every 10 seconds of each video, from its keyframes, about 50 to 100 KB a minute of video (5 to 10 MB for a two-hour film), kept in the data folder (trickplay/) and removed when the file changes or goes. Nothing leaves the server.",
        fields: &[field(
            "MELETE_TRICKPLAY",
            "Make seek-bar previews",
            "The first run reads every video once, one at a time, which takes a while on a large library.",
            Input::Toggle { on: "on" },
        )],
    },
    Service {
        key: "offline_copies",
        name: "Offline copies",
        about: "Converted copies of a film or an episode a device can download and keep, to watch without a connection: made on request in the background, one at a time and only while live conversions leave room, as one MP4 with the subtitles asked for beside it. Kept in the transcode folder (offline/), never in the library: a copy is about as big as its bitrate over its length (some 4.5 GB an hour at 1080p, 2.3 GB at 720p, 1.2 GB at 480p; the size is estimated before it's made), 20 at most per person, and deleted 7 days after its last download, or when the one who asked deletes it. Nothing leaves the server.",
        fields: &[field(
            "MELETE_OFFLINE_COPIES",
            "Let devices ask for offline copies",
            "Copies take room in the transcode folder until they're deleted.",
            Input::Toggle { on: "on" },
        )],
    },
    Service {
        key: "opensubtitles",
        name: "OpenSubtitles",
        about: "Subtitles a file lacks, from OpenSubtitles.com: anyone signed in can search for a language and fetch one of what's found, once per file and language. What leaves the server: the video's OpenSubtitles hash (worked out from its size and its first and last 64 KB, which can't be turned back into the video), its TMDB or IMDb id with the season and episode, the language, and which subtitle to download. Nothing about who asked. The key and the login stay on the server. Fetched subtitles are kept in the data folder (subtitles/fetched/), never in the library. With languages set for new imports, the same is sent without anyone asking for each film or episode Melete imports that has no text subtitle in one of them, once per file and language, and a subtitle is fetched only when it was made for that very file; these count against the day's downloads like any other.",
        fields: &[
            field(
                "MELETE_OPENSUBTITLES_KEY",
                "API key",
                "The key of an app registered at opensubtitles.com (Consumers).",
                Input::Secret,
            ),
            field(
                "MELETE_OPENSUBTITLES_USERNAME",
                "Username",
                "Optional: downloads then count against this account, which allows more of them a day.",
                Input::Text,
            ),
            field(
                "MELETE_OPENSUBTITLES_PASSWORD",
                "Password",
                "",
                Input::Secret,
            ),
            field(
                "MELETE_OPENSUBTITLES_IMPORTS",
                "Fetch for new imports missing",
                "Optional: languages, such as en, fr, pt-br. A film or episode Melete imports without a text subtitle in one of them gets one made for that very file, if OpenSubtitles has it. Empty: only on request.",
                Input::Text,
            ),
        ],
    },
    Service {
        key: "song_albums",
        name: "A song's album",
        about: "Once a requested song is in, want its album too, fetched after everything else.",
        fields: &[field(
            "MELETE_SONG_ALBUMS",
            "Want a song's album after it",
            "",
            Input::Toggle { on: "on" },
        )],
    },
];
