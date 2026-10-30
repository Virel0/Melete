//! Every service as it's set up, and where each value comes from. A secret is only said to be set.

use super::*;

/// Where a setting's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum From {
    /// `.env`, which wins and can't be changed here.
    Environment,
    /// Stored through the web UI.
    Settings,
    /// Neither: the built-in default, or off.
    Default,
}

/// One setting as it's set up now, for the web UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldView {
    /// The environment variable it stands for.
    pub name: &'static str,
    /// What the web UI calls it.
    pub label: &'static str,
    /// A line on what to enter.
    pub hint: &'static str,
    /// How it's entered.
    pub input: Input,
    /// Its value; never a secret's.
    pub value: Option<String>,
    /// Whether it has a value, from either place; the only word on a secret.
    pub set: bool,
    /// Where its value comes from.
    pub from: From,
}

/// One outside service as it's set up now, for the web UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceView {
    /// The service's id in the API, e.g. "tmdb".
    pub key: &'static str,
    /// What the web UI calls it.
    pub name: &'static str,
    /// What it's for, and what leaves the server when it's on.
    pub about: &'static str,
    /// Whether it's in use as things stand.
    pub on: bool,
    /// Whether its Test tries to reach something; a setting of Melete's own has nothing to
    /// connect to.
    pub testable: bool,
    /// The component beside Melete it needs, if any, and whether it's set up.
    pub component: Option<Component>,
    /// Its settings, in the order they're shown.
    pub fields: Vec<FieldView>,
}

/// A component beside Melete that a setting needs, such as Auto DJ the stem separator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Component {
    /// What it is, as a sentence names it: "stem separator".
    pub name: &'static str,
    /// Whether Melete is set up to reach it (its folders are set). Whether it's running
    /// shows only when it's given work.
    pub set_up: bool,
}

/// The component beside Melete that service `key` needs, as `config` sets it up.
fn component(config: &Config, key: &str) -> Option<Component> {
    match key {
        "auto_dj" => Some(Component {
            name: "stem separator",
            set_up: config.stems.is_some(),
        }),
        "motion_covers" => Some(Component {
            name: "motion cover renderer",
            set_up: config.motion_jobs.is_some(),
        }),
        _ => None,
    }
}

pub(super) fn on(config: &Config, key: &str) -> bool {
    match key {
        "tmdb" => config.tmdb_token.is_some(),
        "musicbrainz" => config.musicbrainz,
        "lyrics" => config.lyrics,
        "translation" => config.translation.is_some(),
        "romanisation" => config.romanisation,
        "slskd" => config.slskd.is_some(),
        "downloads" => config.downloads.is_some(),
        "seedbox" => config
            .downloads
            .as_ref()
            .is_some_and(|d| d.seedbox.is_some()),
        "flaresolverr" => config.flaresolverr.is_some(),
        "song_albums" => config.song_albums,
        "auto_dj" => config.auto_dj,
        "motion_covers" => config.motion_covers,
        "intro_detection" => config.intro_detection,
        "trickplay" => config.trickplay,
        "offline_copies" => config.offline_copies,
        // Whole scans always run, on their schedule.
        "scans" => true,
        "discovery" => config.discovery && config.tmdb_token.is_some(),
        "trailers" => config.trailers_online && config.tmdb_token.is_some(),
        "oidc" => config.oidc.is_some(),
        "openrouter" => config.openrouter.is_some(),
        "opensubtitles" => config.opensubtitles.is_some(),
        _ => false,
    }
}

/// Every service as it's set up, `config` being what's running.
pub fn views(
    config: &Config,
    stored: &HashMap<String, String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<ServiceView> {
    SERVICES
        .iter()
        .map(|service| ServiceView {
            key: service.key,
            name: service.name,
            about: service.about,
            on: on(config, service.key),
            testable: crate::service_check::connects(service.key),
            component: component(config, service.key),
            fields: service
                .fields
                .iter()
                .map(|f| {
                    let (value, from) = match (
                        env(f.name).filter(|v| !v.trim().is_empty()),
                        stored.get(f.name),
                    ) {
                        (Some(v), _) => (Some(v), From::Environment),
                        (None, Some(v)) => (Some(v.clone()), From::Settings),
                        (None, None) => (None, From::Default),
                    };
                    FieldView {
                        name: f.name,
                        label: f.label,
                        hint: f.hint,
                        input: f.input,
                        set: value.is_some(),
                        value: value.filter(|_| f.input != Input::Secret),
                        from,
                    }
                })
                .collect(),
        })
        .collect()
}
