//! Settings for the outside services, each off unless its variables are set.

use super::*;

/// A LibreTranslate server, as Melete reaches it.
#[derive(Debug)]
pub struct Translation {
    /// `MELETE_LIBRETRANSLATE_URL`, e.g. http://libretranslate:5000.
    pub url: String,
    /// `MELETE_LIBRETRANSLATE_API_KEY`, for a server that wants one.
    pub api_key: Option<Secret>,
}

/// OpenSubtitles.com, for subtitles a file lacks.
#[derive(Debug, Clone)]
pub struct OpenSubtitles {
    /// `MELETE_OPENSUBTITLES_KEY`, the API key of an app registered with OpenSubtitles.
    pub key: Secret,
    /// `MELETE_OPENSUBTITLES_USERNAME` and `MELETE_OPENSUBTITLES_PASSWORD`, the account
    /// downloads count against; without them they count against the key alone.
    pub login: Option<(String, Secret)>,
    /// `MELETE_OPENSUBTITLES_IMPORTS`, the languages ("en", "pt-br") a new import lacking one
    /// gets a subtitle in without anyone asking; none (the default) fetches only on request.
    pub imports: Vec<String>,
}

/// OpenRouter, for AI features.
#[derive(Debug, Clone)]
pub struct OpenRouter {
    /// `MELETE_OPENROUTER_KEY`, one server-side key so no device holds one.
    pub key: Secret,
    /// `MELETE_OPENROUTER_MODEL`, an OpenRouter model id; its own router picks one by default.
    pub model: String,
}

/// The model when none is set: OpenRouter's own router picks one per request.
pub const DEFAULT_OPENROUTER_MODEL: &str = "openrouter/auto";

/// An OpenID Connect provider users can sign in through.
#[derive(Debug, Clone)]
pub struct Oidc {
    /// `MELETE_OIDC_ISSUER`, exactly as the provider names itself, e.g.
    /// https://auth.example/application/o/melete/.
    pub issuer: String,
    /// `MELETE_OIDC_CLIENT_ID` and `MELETE_OIDC_CLIENT_SECRET`, from registering Melete there.
    pub client_id: String,
    /// Sent only to the provider's token endpoint.
    pub client_secret: Secret,
    /// `MELETE_OIDC_REDIRECT_URL`: Melete's callback as browsers reach it, registered with the
    /// provider, e.g. https://melete.example/api/v1/auth/oidc/callback.
    pub redirect_url: String,
    /// `MELETE_OIDC_NAME`: what the sign-in button calls the provider; its host by default.
    pub name: String,
    /// `MELETE_OIDC_APP_RETURNS`: where apps may be sent back to after signing in, each matched
    /// exactly (separated by spaces or commas). Empty: apps can't sign in through the provider,
    /// except to a port on the device itself (RFC 8252), which needs no listing.
    pub app_returns: Vec<String>,
}

/// Schemes never taken as a place to send an app's sign-in code.
pub(super) const NEVER_RETURN: &[&str] =
    &["javascript", "data", "file", "blob", "about", "vbscript"];

/// The stem separator's folders, as Melete and the separator both see them.
#[derive(Debug, Clone)]
pub struct Stems {
    /// Where the parts are kept (`MELETE_STEMS_DIR`).
    pub dir: PathBuf,
    /// The folder of jobs shared with the separator (`MELETE_STEMS_JOBS`).
    pub jobs: PathBuf,
}

/// slskd's API, as Melete reaches it.
#[derive(Debug)]
pub struct Slskd {
    /// `MELETE_SLSKD_URL`, e.g. http://slskd:5030.
    pub url: String,
    /// `MELETE_SLSKD_API_KEY`: one of the keys in slskd's `web.authentication.api_keys`.
    pub api_key: Secret,
    /// `MELETE_SLSKD_DOWNLOADS`: slskd's downloads folder as mounted in Melete's container;
    /// finished Soulseek downloads wait there until it's set.
    pub downloads: Option<PathBuf>,
}

/// Deluge on the seedbox, through its web UI, and the seedbox's SFTP.
#[derive(Debug)]
pub struct Downloads {
    /// `MELETE_DELUGE_URL`, the web UI as Melete reaches it, e.g. https://host/deluge.
    pub deluge_url: String,
    /// `MELETE_DELUGE_PASSWORD`, the web UI's password.
    pub deluge_password: Secret,
    /// None until `MELETE_SEEDBOX_HOST` is set: downloads are then followed but not fetched.
    pub seedbox: Option<Seedbox>,
}

/// The seedbox's SFTP server.
#[derive(Debug, Clone)]
pub struct Seedbox {
    /// `MELETE_SEEDBOX_HOST` and `MELETE_SEEDBOX_PORT` (default 22).
    pub host: String,
    /// The SFTP port.
    pub port: u16,
    /// `MELETE_SEEDBOX_USER` and `MELETE_SEEDBOX_PASSWORD`.
    pub user: String,
    /// The SFTP password, sent only to the seedbox.
    pub password: Secret,
    /// `MELETE_SEEDBOX_HOST_KEY`: the server's key fingerprint, as OpenSSH writes it
    /// ("SHA256:..."). `melete seedbox host-key <host> <port>` shows it.
    pub host_key: String,
    /// `MELETE_SEEDBOX_PATHS=<deluge folder>=<sftp folder>`: where Deluge's folder for
    /// finished torrents is as SFTP sees it, e.g. /home/user/Completed=/Completed.
    pub deluge_dir: String,
    /// The same folder as SFTP sees it.
    pub sftp_dir: String,
}
