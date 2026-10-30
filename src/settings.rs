//! Outside services set up from the web UI. Each setting is the
//! environment variable it stands for, kept in the database when an admin sets it there. The
//! environment wins, so a setup made in `.env` keeps running as it is; a value set in neither
//! is the built-in default. Secrets are written, never read back: a view says only that one is
//! set. What's saved takes effect without a restart (`live`).

use std::collections::HashMap;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::db::unix_now;

mod catalogue;
mod saving;
mod views;

pub use catalogue::*;
pub use saving::*;
pub use views::*;

fn known(name: &str) -> Option<&'static Field> {
    SERVICES
        .iter()
        .flat_map(|s| s.fields.iter())
        .find(|f| f.name == name)
}

/// Whether a setting is a secret: sealed where it's stored, and never shown back.
pub fn is_secret(name: &str) -> bool {
    known(name).is_some_and(|f| f.input == Input::Secret)
}

/// One setting as it stands: the environment's, else what's stored.
pub async fn value(pool: &SqlitePool, name: &str) -> sqlx::Result<Option<String>> {
    if let Some(set) = std::env::var(name).ok().filter(|v| !v.trim().is_empty()) {
        return Ok(Some(set));
    }
    let stored: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    match stored {
        Some(value) if is_secret(name) => {
            let sealing = crate::secrets::sealing(pool).await?;
            crate::secrets::from_store(sealing, &crate::secrets::setting(name), &value).map(Some)
        }
        other => Ok(other),
    }
}

/// Whether lyrics' pronunciation is on (`MELETE_LYRICS_ROMANISATION`); off by default.
pub async fn romanisation(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_LYRICS_ROMANISATION").await?.as_deref() == Some("on"))
}

/// Whether trending and similar titles are on (`MELETE_DISCOVERY`); off by default.
pub async fn discovery(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_DISCOVERY").await?.as_deref() == Some("on"))
}

/// Whether posters of titles not in the library are served (`MELETE_DISCOVERY_POSTERS`); off by
/// default.
pub async fn discovery_posters(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_DISCOVERY_POSTERS").await?.as_deref() == Some("on"))
}

/// Whether looking for intros and credits is on (`MELETE_INTRO_DETECTION`); off by default.
pub async fn intro_detection(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_INTRO_DETECTION").await?.as_deref() == Some("on"))
}

/// Whether devices may ask for offline copies (`MELETE_OFFLINE_COPIES`); off by default.
pub async fn offline_copies(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_OFFLINE_COPIES").await?.as_deref() == Some("on"))
}

/// Whether the library folders are watched for changes (`MELETE_SCAN_WATCH`); off by default.
pub async fn scan_watch(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_SCAN_WATCH").await?.as_deref() == Some("on"))
}

/// Whether titles' trailers are fetched from TMDB and shown through YouTube's player
/// (`MELETE_TRAILERS_ONLINE`); off by default.
pub async fn trailers_online(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_TRAILERS_ONLINE").await?.as_deref() == Some("on"))
}

/// Whether seek-bar previews are made (`MELETE_TRICKPLAY`); off by default.
pub async fn trickplay(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_TRICKPLAY").await?.as_deref() == Some("on"))
}

/// Whether motion covers are made and shown (`MELETE_MOTION_COVERS`); off by default.
pub async fn motion_covers(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_MOTION_COVERS").await?.as_deref() == Some("on"))
}

/// Whether Auto DJ is on (`MELETE_AUTO_DJ`); off by default.
pub async fn auto_dj(pool: &SqlitePool) -> sqlx::Result<bool> {
    Ok(value(pool, "MELETE_AUTO_DJ").await?.as_deref() == Some("on"))
}

/// Every setting stored through the web UI, by variable name, secrets included: for the server
/// only, never for a response.
pub async fn stored(pool: &SqlitePool) -> sqlx::Result<HashMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT name, value FROM settings")
        .fetch_all(pool)
        .await?;
    let sealing = crate::secrets::sealing(pool).await?;
    rows.into_iter()
        .map(|(name, value)| {
            let value = if is_secret(&name) {
                crate::secrets::from_store(sealing, &crate::secrets::setting(&name), &value)?
            } else {
                value
            };
            Ok((name, value))
        })
        .collect()
}

/// What to store for a setting's value: a secret sealed, in a database that keeps them sealed.
fn to_store(sealing: bool, name: &str, value: &str) -> sqlx::Result<String> {
    if is_secret(name) {
        crate::secrets::to_store(sealing, &crate::secrets::setting(name), value)
    } else {
        Ok(value.to_owned())
    }
}

/// The value each setting takes: the environment's, else what's stored for one the UI sets.
/// `env` stands in for the environment, so tests needn't change the process's.
pub fn lookup<'a>(
    stored: &'a HashMap<String, String>,
    env: &'a (dyn Fn(&str) -> Option<String> + Sync),
) -> impl Fn(&str) -> Option<String> + 'a {
    move |name: &str| match known(name) {
        Some(_) => env(name)
            .filter(|v| !v.trim().is_empty())
            .or_else(|| stored.get(name).cloned()),
        None => env(name),
    }
}

/// The configuration as it stands: the environment, with what the UI set where it's silent.
pub async fn load(pool: &SqlitePool) -> anyhow::Result<Config> {
    let stored = stored(pool).await?;
    let env = |name: &str| std::env::var(name).ok();
    Config::from_env_and(lookup(&stored, &env))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// An environment of these variables only.
    fn env_of(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> + Sync {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    fn change(pairs: &[(&str, Option<&str>)]) -> HashMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_owned)))
            .collect()
    }

    #[test]
    fn the_environment_wins_and_the_ui_fills_in_where_its_silent() {
        let stored: HashMap<String, String> = [
            ("MELETE_TMDB_TOKEN", "from-the-ui"),
            ("MELETE_LYRICS", "lrclib"),
            ("MELETE_BIND", "0.0.0.0:1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let env = env_of(&[("MELETE_TMDB_TOKEN", "from-env"), ("MELETE_LYRICS", "")]);
        let get = lookup(&stored, &env);
        assert_eq!(get("MELETE_TMDB_TOKEN").as_deref(), Some("from-env"));
        assert_eq!(
            get("MELETE_LYRICS").as_deref(),
            Some("lrclib"),
            "an empty one is silent"
        );
        assert_eq!(
            get("MELETE_BIND"),
            None,
            "only what the UI sets is taken from it"
        );
        assert_eq!(get("MELETE_SLSKD_URL"), None);
    }

    /// `.env.example` is copied as it is, and a value in `.env` locks a setting in the web UI: it
    /// may only leave the UI's settings empty or commented out.
    #[test]
    fn the_env_example_leaves_the_uis_settings_to_the_ui() {
        let pinned: Vec<&str> = include_str!("../.env.example")
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .filter(|(name, value)| known(name.trim()).is_some() && !value.trim().is_empty())
            .map(|(name, _)| name)
            .collect();
        assert!(pinned.is_empty(), "set in .env.example: {pinned:?}");
    }

    #[tokio::test]
    async fn saves_what_holds_together_and_refuses_the_rest() {
        let pool = db::open_in_memory().await;
        let env = env_of(&[("MELETE_FLARESOLVERR_URL", "http://flaresolverr:8191")]);
        let refused = |result: anyhow::Result<Result<Config, Refused>>| match result.unwrap() {
            Err(Refused::Invalid(why)) => why,
            other => panic!("{other:?}"),
        };
        assert!(matches!(
            save(&pool, "nothing", &change(&[]), &env).await.unwrap(),
            Err(Refused::NoService)
        ));
        assert!(
            refused(
                save(
                    &pool,
                    "tmdb",
                    &change(&[("MELETE_LYRICS", Some("lrclib"))]),
                    &env
                )
                .await
            )
            .contains("isn't one of TMDB's settings")
        );
        assert!(
            refused(
                save(
                    &pool,
                    "flaresolverr",
                    &change(&[("MELETE_FLARESOLVERR_URL", None)]),
                    &env
                )
                .await
            )
            .contains("set in .env, which wins")
        );
        assert_eq!(
            refused(
                save(
                    &pool,
                    "lyrics",
                    &change(&[("MELETE_LYRICS", Some("on"))]),
                    &env
                )
                .await
            ),
            "MELETE_LYRICS is lrclib or off"
        );
        assert!(
            refused(
                save(
                    &pool,
                    "tmdb",
                    &change(&[("MELETE_TMDB_TOKEN", Some("a\nb"))]),
                    &env
                )
                .await
            )
            .contains("one line")
        );
        // Downloads on, with nowhere to download to: the configuration says what's missing.
        let why = refused(
            save(
                &pool,
                "downloads",
                &change(&[("MELETE_DOWNLOADS", Some("on"))]),
                &env,
            )
            .await,
        );
        assert!(why.contains("MELETE_DELUGE_URL"), "{why}");
        assert!(
            !stored(&pool)
                .await
                .unwrap()
                .contains_key("MELETE_DOWNLOADS"),
            "nothing kept"
        );

        let config = save(
            &pool,
            "downloads",
            &change(&[
                ("MELETE_DOWNLOADS", Some("on")),
                (
                    "MELETE_DELUGE_URL",
                    Some(" https://seedbox.example/deluge "),
                ),
                ("MELETE_DELUGE_PASSWORD", Some("secret")),
            ]),
            &env,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            config.downloads.unwrap().deluge_url,
            "https://seedbox.example/deluge"
        );
        let kept = stored(&pool).await.unwrap();
        assert_eq!(kept["MELETE_DELUGE_PASSWORD"], "secret");
        // MusicBrainz is off by default: nothing is kept for it until an admin sets it.
        assert!(!kept.contains_key("MELETE_MUSICBRAINZ"));
        save(
            &pool,
            "musicbrainz",
            &change(&[("MELETE_MUSICBRAINZ", Some("off"))]),
            &env,
        )
        .await
        .unwrap()
        .unwrap();
        save(
            &pool,
            "downloads",
            &change(&[
                ("MELETE_DELUGE_PASSWORD", None),
                ("MELETE_DOWNLOADS", Some("")),
            ]),
            &env,
        )
        .await
        .unwrap()
        .unwrap();
        let kept = stored(&pool).await.unwrap();
        assert_eq!(
            kept.get("MELETE_MUSICBRAINZ").map(String::as_str),
            Some("off")
        );
        assert!(
            !kept.contains_key("MELETE_DELUGE_PASSWORD") && !kept.contains_key("MELETE_DOWNLOADS")
        );

        // Put back as it was, after a change that couldn't be set up.
        let before = kept.clone();
        save(
            &pool,
            "tmdb",
            &change(&[("MELETE_TMDB_TOKEN", Some("new"))]),
            &env,
        )
        .await
        .unwrap()
        .unwrap();
        restore(&pool, ["MELETE_TMDB_TOKEN".to_owned()].iter(), &before)
            .await
            .unwrap();
        assert_eq!(stored(&pool).await.unwrap(), before);
    }

    #[tokio::test]
    async fn a_setting_locked_by_env_names_its_variable_and_the_way_out() {
        let pool = db::open_in_memory().await;
        let env = env_of(&[("MELETE_MUSICBRAINZ", "on")]);
        let why = match save(
            &pool,
            "musicbrainz",
            &change(&[("MELETE_MUSICBRAINZ", Some("off"))]),
            &env,
        )
        .await
        .unwrap()
        {
            Err(Refused::Invalid(why)) => why,
            other => panic!("{other:?}"),
        };
        assert!(
            why.starts_with("MELETE_MUSICBRAINZ is set in .env"),
            "{why}"
        );
        assert!(
            why.contains("remove that line from .env and restart Melete"),
            "{why}"
        );
    }

    #[tokio::test]
    async fn a_view_says_a_secret_is_set_but_never_shows_it() {
        let pool = db::open_in_memory().await;
        let env = env_of(&[("MELETE_SLSKD_URL", "http://slskd:5030")]);
        save(
            &pool,
            "slskd",
            &change(&[("MELETE_SLSKD_API_KEY", Some("key-value"))]),
            &env,
        )
        .await
        .unwrap()
        .unwrap();
        let config = Config::from_env_and(lookup(&stored(&pool).await.unwrap(), &env)).unwrap();
        let views = views(&config, &stored(&pool).await.unwrap(), &env);
        let slskd = views.iter().find(|s| s.key == "slskd").unwrap();
        assert!(slskd.on);
        let fields: Vec<(&str, Option<&str>, bool, From)> = slskd
            .fields
            .iter()
            .map(|f| (f.name, f.value.as_deref(), f.set, f.from))
            .collect();
        assert_eq!(
            fields,
            [
                (
                    "MELETE_SLSKD_URL",
                    Some("http://slskd:5030"),
                    true,
                    From::Environment
                ),
                ("MELETE_SLSKD_API_KEY", None, true, From::Settings),
                ("MELETE_SLSKD_DOWNLOADS", None, false, From::Default),
            ]
        );
        assert!(!serde_json::to_string(&views).unwrap().contains("key-value"));
        let tmdb = views.iter().find(|s| s.key == "tmdb").unwrap();
        assert!(!tmdb.on);
    }

    #[tokio::test]
    async fn intro_detection_is_off_until_turned_on() {
        let pool = db::open_in_memory().await;
        assert!(!intro_detection(&pool).await.unwrap());
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_INTRO_DETECTION', 'on', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(intro_detection(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn auto_dj_is_off_until_turned_on() {
        let pool = db::open_in_memory().await;
        assert!(!auto_dj(&pool).await.unwrap());
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_AUTO_DJ', 'on', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(auto_dj(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn motion_covers_are_off_until_turned_on() {
        let pool = db::open_in_memory().await;
        assert!(!motion_covers(&pool).await.unwrap());
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_MOTION_COVERS', 'on', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(motion_covers(&pool).await.unwrap());
    }
}
