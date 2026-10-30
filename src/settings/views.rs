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
    /// The service it only works with, by key, when there's one: it's shown under that one, and
    /// stays off while that one is.
    pub needs: Option<&'static str>,
    /// Its settings, in the order they're shown.
    pub fields: Vec<FieldView>,
}

/// The service a feature only works with, by key: trending and similar titles and trailers
/// online ask TMDB for what they show.
pub fn needs(key: &str) -> Option<&'static str> {
    match key {
        "discovery" | "trailers" => Some("tmdb"),
        _ => None,
    }
}

/// Whether a service is in use, leaving aside the one it needs.
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
        "discovery" => config.discovery,
        "trailers" => config.trailers_online,
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
            on: on(config, service.key) && needs(service.key).is_none_or(|n| on(config, n)),
            needs: needs(service.key),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The views with these settings in the environment and nothing stored.
    fn with(pairs: &'static [(&'static str, &'static str)]) -> Vec<ServiceView> {
        let env = move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        };
        let config = Config::from_env_and(env).unwrap();
        views(&config, &HashMap::new(), &env)
    }

    /// What a service needs, and whether it's on.
    fn of(views: &[ServiceView], key: &str) -> (Option<&'static str>, bool) {
        let view = views.iter().find(|s| s.key == key).unwrap();
        (view.needs, view.on)
    }

    #[test]
    fn what_needs_tmdb_says_so_and_is_off_without_its_token() {
        let without = with(&[("MELETE_DISCOVERY", "on"), ("MELETE_TRAILERS_ONLINE", "on")]);
        assert_eq!(of(&without, "discovery"), (Some("tmdb"), false));
        assert_eq!(of(&without, "trailers"), (Some("tmdb"), false));
        assert_eq!(of(&without, "tmdb"), (None, false));
        assert_eq!(of(&without, "musicbrainz").0, None);

        let token = with(&[
            ("MELETE_DISCOVERY", "on"),
            ("MELETE_TRAILERS_ONLINE", "on"),
            ("MELETE_TMDB_TOKEN", "token"),
        ]);
        assert_eq!(of(&token, "discovery"), (Some("tmdb"), true));
        assert_eq!(of(&token, "trailers"), (Some("tmdb"), true));
        // Each service needed is one there is, and needs nothing itself.
        for view in &token {
            if let Some(needed) = view.needs {
                assert_eq!(of(&token, needed).0, None, "{}", view.key);
            }
        }
    }
}
