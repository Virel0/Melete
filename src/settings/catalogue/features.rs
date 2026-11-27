//! Melete's own features, set up on the Features page: they talk to nothing outside the server.

use super::*;

pub(super) const AUTO_DJ: Service = Service {
    key: "auto_dj",
    name: "Auto DJ",
    group: Group::Music,
    purpose: "Mixing one track into the next",
    about: "Mixing from one track into the next, like a DJ: Melete splits music into its parts (voice, drums, bass, guitar, other) with the stem separator and plans the transitions. Nothing leaves the server.",
    fields: &[field(
        "MELETE_AUTO_DJ",
        "Turn on Auto DJ",
        "Needs the stem separator (compose.stems.yaml) to split music into its parts.",
        Input::Toggle { on: "on" },
    )],
};

pub(super) const SCANS: Service = Service {
    key: "scans",
    name: "Library scans",
    group: Group::Library,
    purpose: "How often libraries are scanned whole",
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
};

pub(super) const INTRO_DETECTION: Service = Service {
    key: "intro_detection",
    name: "Intros and credits",
    group: Group::Playback,
    purpose: "Skipping intros and credits",
    about: "Finding intros and credits, so players can offer to skip them: each episode's opening and end are compared with others of its season, and the end of a movie, or of an episode whose credits that didn't find, is looked at alone for a long run of black or silent seconds. Nothing leaves the server.",
    fields: &[field(
        "MELETE_INTRO_DETECTION",
        "Look for intros and credits",
        "The first run reads part of every episode's audio and the end of every movie, which takes a while on a large library.",
        Input::Toggle { on: "on" },
    )],
};

pub(super) const TRICKPLAY: Service = Service {
    key: "trickplay",
    name: "Seek-bar previews",
    group: Group::Playback,
    purpose: "Pictures on the seek bar",
    about: "Small pictures on the seek bar, so you can see where a seek lands: one every 10 seconds of each video, from its keyframes, about 50 to 100 KB a minute of video at 320 pixels wide (5 to 10 MB for a two-hour film), kept in the data folder (trickplay/) and removed when the file changes or goes. Nothing leaves the server.",
    fields: &[
        field(
            "MELETE_TRICKPLAY",
            "Make seek-bar previews",
            "The first run reads every video once, one at a time, which takes a while on a large library.",
            Input::Toggle { on: "on" },
        ),
        field(
            crate::trickplay::WIDTH_SETTING,
            "Preview width",
            "Wider pictures look sharper on large screens and take more room. A change makes every preview again in the background, one file at a time; the old ones are shown until then.",
            Input::Choice {
                options: crate::trickplay::WIDTH_CHOICES,
            },
        ),
    ],
};

pub(super) const OFFLINE_COPIES: Service = Service {
    key: "offline_copies",
    name: "Offline copies",
    group: Group::Playback,
    purpose: "Copies devices keep to watch offline",
    about: "Converted copies of a film or an episode a device can download and keep, to watch without a connection: made on request in the background, one at a time and only while live conversions leave room, as one MP4 with the subtitles asked for beside it. Kept in the transcode folder (offline/), never in the library: a copy is about as big as its bitrate over its length (some 4.5 GB an hour at 1080p, 2.3 GB at 720p, 1.2 GB at 480p; the size is estimated before it's made), 20 at most per person, and deleted 7 days after its last download, or when the one who asked deletes it. Nothing leaves the server.",
    fields: &[field(
        "MELETE_OFFLINE_COPIES",
        "Let devices ask for offline copies",
        "Copies take room in the transcode folder until they're deleted.",
        Input::Toggle { on: "on" },
    )],
};
