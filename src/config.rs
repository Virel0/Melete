//! Melete's settings, from `MELETE_*` environment variables. Each outside service is off
//! unless its variables are set, and secrets are held in [`Secret`], which `Debug` output
//! never shows.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, bail};

use crate::library::{LibraryCategory, LibraryConfig, LibraryKind};
use crate::transcode::Hardware;

mod libraries;
mod reading;
mod services;

use libraries::*;
pub use reading::*;
pub use services::*;

const DEFAULT_BIND: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 8100);

/// Settings read from `MELETE_*` environment variables.
#[derive(Debug)]
pub struct Config {
    /// The address and port the HTTP server listens on (`MELETE_BIND`, default 0.0.0.0:8100).
    pub bind: SocketAddr,
    /// Where `melete.db` lives (`MELETE_DATA_DIR`, default `/config`).
    pub data_dir: PathBuf,
    /// One per `MELETE_LIBRARY_<NAME>=<kind>:<path>` variable, sorted by name.
    pub libraries: Vec<LibraryConfig>,
    /// The libraries given a category (`MELETE_LIBRARY_<NAME>=<kind>,<category>:<path>`), by name.
    pub library_categories: Vec<(String, LibraryCategory)>,
    /// TMDB API Read Access Token (`MELETE_TMDB_TOKEN`); metadata is off without it.
    pub tmdb_token: Option<Secret>,
    /// A FlareSolverr (`MELETE_FLARESOLVERR_URL`, e.g. http://flaresolverr:8191), for trackers
    /// behind Cloudflare's browser check (Uindex); those are off without it.
    pub flaresolverr: Option<String>,
    /// Matching music to MusicBrainz (`MELETE_MUSICBRAINZ=on`; off by default, as every outside
    /// service is until it's configured).
    pub musicbrainz: bool,
    /// Synced lyrics for imported albums from LRCLIB (`MELETE_LYRICS=lrclib`; off by default,
    /// since each track's title, artist, album and length leave the server).
    pub lyrics: bool,
    /// Once a requested song is in, its album is wanted too, after other work
    /// (`MELETE_SONG_ALBUMS=on`; off by default).
    pub song_albums: bool,
    /// Mixing into the next track like a DJ, with music split into its parts
    /// (`MELETE_AUTO_DJ=on`; off by default).
    pub auto_dj: bool,
    /// Looking for episodes' intros and credits (`MELETE_INTRO_DETECTION=on`; off by default,
    /// as the first run decodes part of every episode).
    pub intro_detection: bool,
    /// Making seek-bar previews (`MELETE_TRICKPLAY=on`; off by default, as the first run reads
    /// every video once).
    pub trickplay: bool,
    /// Letting devices ask for offline copies (`MELETE_OFFLINE_COPIES=on`; off by default, as
    /// copies take room in the transcode folder).
    pub offline_copies: bool,
    /// Trending and similar titles from TMDB (`MELETE_DISCOVERY=on`; off by default).
    pub discovery: bool,
    /// Titles' trailers from TMDB's video lists, played through YouTube's privacy-enhanced
    /// player (`MELETE_TRAILERS_ONLINE=on`; off by default).
    pub trailers_online: bool,
    /// The language TMDB's titles, summaries, episode names and logos are kept in
    /// (`MELETE_METADATA_LANGUAGE`, such as `fr-FR`; `en-US` by default), English filling what
    /// it hasn't translated.
    pub metadata_language: String,
    /// Splitting music into its parts: where the parts are kept (`MELETE_STEMS_DIR`) and
    /// the folder of jobs shared with the separator (`MELETE_STEMS_JOBS`); off unless both are set.
    pub stems: Option<Stems>,
    /// Motion covers made from still covers: the folder of jobs shared with the renderer
    /// (`MELETE_MOTION_JOBS`); off unless set. The videos are kept under `data_dir`.
    pub motion_jobs: Option<PathBuf>,
    /// Making motion covers for albums without one (`MELETE_MOTION_COVERS=on`; off by default).
    pub motion_covers: bool,
    /// Which parts of Melete each way of reaching it opens (`MELETE_WAYS_IN`), and the
    /// reverse proxies trusted to say who their clients are (`MELETE_TRUSTED_PROXIES`).
    pub ways_in: crate::ways_in::WaysIn,
    /// The ffprobe binary (`MELETE_FFPROBE`); defaults to the one jellyfin-ffmpeg installs.
    pub ffprobe: PathBuf,
    /// The ffmpeg binary (`MELETE_FFMPEG`), likewise.
    pub ffmpeg: PathBuf,
    /// `MELETE_HWACCEL`: `nvenc` for the NVIDIA GPU, `none` (default) for the CPU.
    pub hwaccel: Hardware,
    /// Where transcoded segments go (`MELETE_TRANSCODE_DIR`, default `/transcode`).
    pub transcode_dir: PathBuf,
    /// Transcodes running at once (`MELETE_MAX_TRANSCODES`, default 4).
    pub max_transcodes: usize,
    /// Downloading (`MELETE_DOWNLOADS=on`; off by default): where Deluge is.
    pub downloads: Option<Downloads>,
    /// Soulseek through slskd (`MELETE_SLSKD_URL`; off when empty).
    pub slskd: Option<Slskd>,
    /// Translating lyrics through LibreTranslate (`MELETE_LIBRETRANSLATE_URL`; off when empty).
    pub translation: Option<Translation>,
    /// Lyrics' pronunciation in Latin letters (`MELETE_LYRICS_ROMANISATION=on`; off by default).
    pub romanisation: bool,
    /// Signing in through an outside identity provider (`MELETE_OIDC_*`; off unless set).
    pub oidc: Option<Oidc>,
    /// AI features through OpenRouter (`MELETE_OPENROUTER_KEY`; off unless set).
    pub openrouter: Option<OpenRouter>,
    /// Subtitles fetched from OpenSubtitles (`MELETE_OPENSUBTITLES_KEY`; off unless set).
    pub opensubtitles: Option<OpenSubtitles>,
}

/// A value kept out of `Debug` output, so logging the config cannot leak it.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// The value itself. Never log it, put it in a response or an error.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// A secret holding a value an outside service gave, such as a login token.
    pub fn new(value: &str) -> Self {
        Secret(value.into())
    }

    /// A secret holding `value`, for tests.
    #[cfg(test)]
    pub fn for_tests(value: &str) -> Self {
        Secret(value.into())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Config {
    /// Where the database is (`MELETE_DATA_DIR`), all that's needed before the rest of the
    /// settings can be read, some of them being in it.
    pub fn data_dir_from_env() -> PathBuf {
        std::env::var("MELETE_DATA_DIR").map_or_else(|_| PathBuf::from("/config"), PathBuf::from)
    }

    /// The configuration with `lookup`'s settings (the environment's, or with what the web UI
    /// set where it's silent: `settings::lookup`) and the environment's libraries.
    pub fn from_env_and(lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let mut config = Self::from_lookup(lookup)?;
        let libraries = parse_library_vars(
            std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
        )?;
        config.library_categories = libraries
            .iter()
            .filter_map(|(library, category)| Some((library.name.clone(), (*category)?)))
            .collect();
        config.libraries = libraries.into_iter().map(|(library, _)| library).collect();
        Ok(config)
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let bind = bind(&get)?;
        let data_dir =
            get("MELETE_DATA_DIR").map_or_else(|| PathBuf::from("/config"), PathBuf::from);
        let tmdb_token = get("MELETE_TMDB_TOKEN")
            .filter(|token| !token.is_empty())
            .map(Secret);
        let flaresolverr = flaresolverr(&get)?;
        let slskd = slskd(&get)?;
        let translation = translation(&get)?;
        let musicbrainz = switch(&get, "MELETE_MUSICBRAINZ")?;
        let lyrics = match get("MELETE_LYRICS").as_deref() {
            None | Some("" | "off") => false,
            Some("lrclib") => true,
            Some(other) => anyhow::bail!("MELETE_LYRICS={other:?} should be lrclib or off"),
        };
        let ways_in = ways_in(&get)?;
        let stems = stems(&get)?;
        let motion_jobs = get("MELETE_MOTION_JOBS")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let motion_covers = switch(&get, "MELETE_MOTION_COVERS")?;
        let auto_dj = switch(&get, "MELETE_AUTO_DJ")?;
        let romanisation = switch(&get, "MELETE_LYRICS_ROMANISATION")?;
        let intro_detection = switch(&get, "MELETE_INTRO_DETECTION")?;
        let discovery = switch(&get, "MELETE_DISCOVERY")?;
        let trailers_online = switch(&get, "MELETE_TRAILERS_ONLINE")?;
        let metadata_language = metadata_language(&get)?;
        check_scan_schedule(&get)?;
        // Watching the folders: read where it's used (`settings`), as it can change while
        // running, and checked here.
        switch(&get, "MELETE_SCAN_WATCH")?;
        // AI picks: read where they're made (`ai_picks`), as they can change while running.
        switch(&get, "MELETE_AI_PICKS")?;
        check_ratings_country(&get)?;
        // Posters of titles not in the library: read where they're served (`settings`), checked
        // here.
        if let Some(other) =
            get("MELETE_DISCOVERY_POSTERS").filter(|v| !matches!(v.as_str(), "" | "on" | "off"))
        {
            anyhow::bail!("MELETE_DISCOVERY_POSTERS={other:?} should be on or off");
        }
        let trickplay = switch(&get, "MELETE_TRICKPLAY")?;
        check_trickplay_width(&get)?;
        let offline_copies = switch(&get, "MELETE_OFFLINE_COPIES")?;
        let song_albums = switch(&get, "MELETE_SONG_ALBUMS")?;
        let ffprobe = get("MELETE_FFPROBE").map_or_else(
            || PathBuf::from("/usr/lib/jellyfin-ffmpeg/ffprobe"),
            PathBuf::from,
        );
        let ffmpeg = get("MELETE_FFMPEG").map_or_else(
            || PathBuf::from("/usr/lib/jellyfin-ffmpeg/ffmpeg"),
            PathBuf::from,
        );
        let hwaccel = hwaccel(&get)?;
        let transcode_dir =
            get("MELETE_TRANSCODE_DIR").map_or_else(|| PathBuf::from("/transcode"), PathBuf::from);
        let max_transcodes = max_transcodes(&get)?;
        let downloads = downloads(&get)?;
        let oidc = oidc(&get)?;
        let openrouter = openrouter(&get)?;
        let opensubtitles = opensubtitles(&get)?;
        Ok(Self {
            bind,
            data_dir,
            libraries: Vec::new(),
            library_categories: Vec::new(),
            tmdb_token,
            flaresolverr,
            musicbrainz,
            lyrics,
            song_albums,
            auto_dj,
            intro_detection,
            trickplay,
            offline_copies,
            discovery,
            trailers_online,
            metadata_language,
            stems,
            motion_jobs,
            motion_covers,
            ways_in,
            ffprobe,
            ffmpeg,
            hwaccel,
            transcode_dir,
            max_transcodes,
            downloads,
            slskd,
            translation,
            romanisation,
            oidc,
            openrouter,
            opensubtitles,
        })
    }
}

/// The address to listen on (`MELETE_BIND`), all interfaces on port 8100 by default.
fn bind(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<SocketAddr> {
    Ok(match get("MELETE_BIND") {
        Some(value) => value.parse().with_context(|| {
            format!("MELETE_BIND={value:?} is not an address like 0.0.0.0:8100")
        })?,
        None => DEFAULT_BIND,
    })
}

/// A setting that is on or off: off when unset or empty, refused when it's anything else.
fn switch(get: &impl Fn(&str) -> Option<String>, key: &str) -> anyhow::Result<bool> {
    Ok(match get(key).as_deref() {
        None | Some("" | "off") => false,
        Some("on") => true,
        Some(other) => anyhow::bail!("{key}={other:?} should be on or off"),
    })
}

/// The ways in (`MELETE_WAYS_IN`) and the proxies trusted to name their clients.
fn ways_in(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<crate::ways_in::WaysIn> {
    let ways = match get("MELETE_WAYS_IN").filter(|v| !v.trim().is_empty()) {
        Some(text) => Some(
            crate::ways_in::parse_ways(&text)
                .map_err(|why| anyhow::anyhow!("MELETE_WAYS_IN: {why}"))?,
        ),
        None => None,
    };
    let proxies = crate::ways_in::parse_nets(&get("MELETE_TRUSTED_PROXIES").unwrap_or_default())
        .map_err(|why| anyhow::anyhow!("MELETE_TRUSTED_PROXIES: {why}"))?;
    Ok(crate::ways_in::WaysIn { ways, proxies })
}

/// The stem separator's two folders: both or neither.
fn stems(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Stems>> {
    Ok(
        match (
            get("MELETE_STEMS_DIR").filter(|v| !v.is_empty()),
            get("MELETE_STEMS_JOBS").filter(|v| !v.is_empty()),
        ) {
            (Some(dir), Some(jobs)) => Some(Stems {
                dir: PathBuf::from(dir),
                jobs: PathBuf::from(jobs),
            }),
            (None, None) => None,
            _ => bail!("MELETE_STEMS_DIR and MELETE_STEMS_JOBS go together: set both, or neither"),
        },
    )
}

/// What transcodes run on (`MELETE_HWACCEL`), the CPU by default.
fn hwaccel(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Hardware> {
    Ok(match get("MELETE_HWACCEL") {
        Some(value) => Hardware::parse(&value)
            .with_context(|| format!("MELETE_HWACCEL={value:?} should be nvenc or none"))?,
        None => Hardware::Software,
    })
}

/// How many transcodes may run at once (`MELETE_MAX_TRANSCODES`): 1 to 32, 4 by default.
fn max_transcodes(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<usize> {
    Ok(match get("MELETE_MAX_TRANSCODES") {
        Some(value) => value
            .parse()
            .ok()
            .filter(|n| (1..=32).contains(n))
            .with_context(|| format!("MELETE_MAX_TRANSCODES={value:?} should be 1 to 32"))?,
        None => 4,
    })
}

#[cfg(test)]
mod tests;
