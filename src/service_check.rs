//! Testing an outside service's connection before or after saving its settings: does it
//! answer, take the key or password, and (for slskd) sign in and share nothing. Each answer is
//! in plain words. A test asks the service for nothing about the household: TMDB checks the
//! token, MusicBrainz looks up a placeholder artist, slskd and Deluge say how they are, the
//! seedbox lists its folder, FlareSolverr lists its sessions.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::{deluge, flaresolverr, libretranslate, musicbrainz, seedbox, slskd, tmdb};

/// MusicBrainz's "Various Artists": an artist that's always there, and tells nothing.
const PLACEHOLDER_ARTIST: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

/// How an outside service answered a check, for an admin to read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Checked {
    /// Whether it answered as it should.
    pub ok: bool,
    /// What happened, in plain words; never a secret.
    pub message: String,
}

fn ok(message: impl Into<String>) -> Checked {
    Checked {
        ok: true,
        message: message.into(),
    }
}

fn wrong(message: impl Into<String>) -> Checked {
    Checked {
        ok: false,
        message: message.into(),
    }
}

/// Whether service `key`'s Test tries to reach something; the rest are settings of Melete's own,
/// with nothing to connect to.
pub fn connects(key: &str) -> bool {
    matches!(
        key,
        "tmdb"
            | "musicbrainz"
            | "lyrics"
            | "slskd"
            | "downloads"
            | "seedbox"
            | "flaresolverr"
            | "translation"
            | "oidc"
            | "openrouter"
            | "opensubtitles"
    )
}

/// The Test's answer for a setting of Melete's own.
const NOTHING_TO_CONNECT_TO: &str =
    "There's nothing to connect to: it's a setting of Melete's own.";

/// Tests service `key` as `config` sets it up.
pub async fn check(pool: &SqlitePool, key: &str, config: &Config) -> anyhow::Result<Checked> {
    Ok(match key {
        "tmdb" => match &config.tmdb_token {
            None => wrong("There's no token to test yet."),
            Some(token) => tmdb_takes(token).await?,
        },
        "musicbrainz" => match config.musicbrainz {
            false => wrong("It's off."),
            true => match musicbrainz::Client::new()?.artist(PLACEHOLDER_ARTIST).await {
                Ok(_) => ok("MusicBrainz answered."),
                Err(err) => wrong(format!("MusicBrainz didn't answer: {err}")),
            },
        },
        "lyrics" => match config.lyrics {
            false => wrong("It's off."),
            true => ok(
                "Lyrics are looked up as music is imported or an app asks; there's nothing to connect to before.",
            ),
        },
        "slskd" => match &config.slskd {
            None => wrong("Set slskd's address and an API key first."),
            Some(settings) => slskd_answers(settings).await?,
        },
        "downloads" => match &config.downloads {
            None => wrong("Turn downloads on and set Deluge's web UI first."),
            Some(settings) => deluge_answers(settings).await?,
        },
        "seedbox" => match config.downloads.as_ref().and_then(|d| d.seedbox.clone()) {
            None => wrong(
                "Set the seedbox's host, user, password, host key and folders, with downloads on.",
            ),
            Some(settings) => seedbox_answers(pool, settings).await?,
        },
        "flaresolverr" => match &config.flaresolverr {
            None => wrong("There's no address to test yet."),
            Some(url) => match flaresolverr::Client::new(url)?.ready().await {
                Ok(solver @ flaresolverr::Solver::FlareSolverr(_)) => {
                    ok(format!("{solver} is ready."))
                }
                Ok(flaresolverr::Solver::Compatible) => ok(
                    "A FlareSolverr-compatible solver answered. It doesn't list its sessions, so whether it gets past a check shows on the first search through it.",
                ),
                Err(why) => wrong(why),
            },
        },
        "translation" => match &config.translation {
            None => wrong("There's no address to test yet."),
            Some(settings) => libretranslate_answers(settings).await?,
        },
        "oidc" => match &config.oidc {
            None => wrong("Set the issuer, client id, client secret and callback address first."),
            Some(settings) => match crate::oidc::Client::new(settings.clone())?.check().await {
                Ok(()) => ok(
                    "The provider answered and names itself as set. Sign in through it once to be sure the client id, secret and callback address are right.",
                ),
                Err(err) => wrong(format!("{err:#}")),
            },
        },
        "openrouter" => match &config.openrouter {
            None => wrong("There's no key to test yet."),
            Some(settings) => match crate::ai::Client::new(settings)?.check().await {
                Ok(()) => ok("OpenRouter took the key. Nothing was sent to a model."),
                Err(failure) => wrong(format!("{failure}.")),
            },
        },
        "opensubtitles" => match &config.opensubtitles {
            None => wrong("There's no API key to test yet."),
            Some(settings) => match crate::opensubtitles::Client::new(settings)?.check().await {
                Ok(()) if settings.login.is_some() => ok(
                    "OpenSubtitles took the key and the login. Nothing was searched or downloaded.",
                ),
                Ok(()) => ok("OpenSubtitles took the key. Nothing was searched or downloaded."),
                Err(failure) => wrong(format!("{failure}.")),
            },
        },
        _ => ok(NOTHING_TO_CONNECT_TO),
    })
}

/// Whether TMDB takes the token.
async fn tmdb_takes(token: &crate::config::Secret) -> anyhow::Result<Checked> {
    Ok(
        match tmdb::Client::new(token.expose().to_owned())?.check().await {
            Ok(()) => ok("TMDB took the token."),
            Err(err) if err.is::<tmdb::Unauthorized>() => wrong(
                "TMDB turned the token down. Use the API Read Access Token (the long one), not the API key.",
            ),
            Err(err) => wrong(format!("TMDB didn't answer: {err}")),
        },
    )
}

/// Whether slskd answers with the key, is signed in to Soulseek, and shares nothing.
async fn slskd_answers(settings: &crate::config::Slskd) -> anyhow::Result<Checked> {
    let client = slskd::Client::new(&settings.url, settings.api_key.clone())?;
    Ok(match client.status().await {
        Err(err) if err.is::<slskd::TurnedDown>() => wrong(format!("{err}.")),
        Err(err) => wrong(format!(
            "Couldn't reach slskd with that address and key: {err}"
        )),
        Ok(slskd::Status { signed_in, .. }) => match client.ensure_not_sharing().await {
            Err(err) => wrong(format!("{err}.")),
            Ok(()) if signed_in => {
                ok("slskd answered, is signed in to Soulseek, and shares nothing.")
            }
            Ok(()) => wrong(
                "slskd answered and shares nothing, but isn't signed in to Soulseek: check its Soulseek account and the VPN.",
            ),
        },
    })
}

/// Whether Deluge's web UI takes the password and is connected to its daemon.
async fn deluge_answers(settings: &crate::config::Downloads) -> anyhow::Result<Checked> {
    let client = deluge::Client::new(&settings.deluge_url, settings.deluge_password.clone())?;
    Ok(match client.check().await {
        Ok(()) => ok("Signed in to Deluge, and it's connected to its daemon."),
        Err(err) => wrong(format!("Deluge: {err}")),
    })
}

/// Whether the seedbox's SFTP connects with the pinned host key, and how full its folder is.
async fn seedbox_answers(
    pool: &SqlitePool,
    settings: crate::config::Seedbox,
) -> anyhow::Result<Checked> {
    let deluge_paths: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT save_path FROM downloads
                     WHERE state = 'completed' AND save_path IS NOT NULL ORDER BY save_path",
    )
    .fetch_all(pool)
    .await?;
    Ok(
        match seedbox::Seedbox::new(settings).test(&deluge_paths).await {
            Ok(report) => ok(format!(
                "Connected, and the host key matched. {} has {} {}.",
                report.folder,
                report.entries,
                if report.entries == 1 {
                    "entry"
                } else {
                    "entries"
                }
            )),
            Err(err) => wrong(format!("The seedbox: {err}")),
        },
    )
}

/// Whether LibreTranslate answers, has languages, and takes the key.
async fn libretranslate_answers(settings: &crate::config::Translation) -> anyhow::Result<Checked> {
    let client = libretranslate::Client::new(&settings.url, settings.api_key.clone())?;
    Ok(match client.languages().await {
        Err(err) => wrong(format!("LibreTranslate didn't answer: {err}")),
        Ok(languages) if languages.is_empty() => {
            wrong("LibreTranslate answered, but has no languages.")
        }
        Ok(languages) => {
            // A word that says nothing, to check the key's taken.
            let target = languages
                .iter()
                .find(|l| *l == "en")
                .unwrap_or(&languages[0]);
            match client.translate(&["Hello".to_owned()], target).await {
                Ok(_) => ok(format!(
                    "LibreTranslate answered, translating into {} languages.",
                    languages.len()
                )),
                Err(err) => wrong(format!("{err}")),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::db;

    fn config(pairs: &[(&str, String)]) -> Config {
        let set: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        Config::from_env_and(|name| set.get(name).cloned()).unwrap()
    }

    async fn said(key: &str, config: &Config) -> Checked {
        let pool = db::open_in_memory().await;
        check(&pool, key, config).await.unwrap()
    }

    #[tokio::test]
    async fn says_what_to_set_before_anything_is_tried() {
        let none = config(&[]);
        for (key, message) in [
            ("tmdb", "There's no token to test yet."),
            ("musicbrainz", "It's off."),
            ("lyrics", "It's off."),
            ("slskd", "Set slskd's address and an API key first."),
            (
                "downloads",
                "Turn downloads on and set Deluge's web UI first.",
            ),
            ("flaresolverr", "There's no address to test yet."),
        ] {
            assert_eq!(said(key, &none).await, wrong(message), "{key}");
        }
        assert!(
            said("seedbox", &none)
                .await
                .message
                .starts_with("Set the seedbox's host")
        );
        assert!(said("song_albums", &none).await.ok);
    }

    #[tokio::test]
    async fn slskd_must_answer_be_signed_in_and_share_nothing() {
        let (_, seen) = crate::slskd::testing::start(serde_json::json!([]), 1).await;
        let base = seen.lock().unwrap().base.clone();
        let with_key = |key: &str| {
            config(&[
                ("MELETE_SLSKD_URL", base.clone()),
                ("MELETE_SLSKD_API_KEY", key.to_owned()),
            ])
        };
        let good = with_key(crate::slskd::testing::KEY);
        assert_eq!(
            said("slskd", &good).await,
            ok("slskd answered, is signed in to Soulseek, and shares nothing.")
        );
        assert!(!said("slskd", &with_key("wrong")).await.ok);
        seen.lock().unwrap().signed_out = true;
        assert!(
            said("slskd", &good)
                .await
                .message
                .contains("isn't signed in to Soulseek")
        );
        seen.lock().unwrap().shared = 2;
        assert!(
            said("slskd", &good)
                .await
                .message
                .contains("shares 2 folder(s)")
        );
    }

    #[tokio::test]
    async fn slskd_says_which_role_its_key_needs() {
        let (_, seen) = crate::slskd::testing::start(serde_json::json!([]), 1).await;
        let base = seen.lock().unwrap().base.clone();
        let settings = config(&[
            ("MELETE_SLSKD_URL", base),
            (
                "MELETE_SLSKD_API_KEY",
                crate::slskd::testing::KEY.to_owned(),
            ),
        ]);
        assert!(said("slskd", &settings).await.ok);
        assert_eq!(
            seen.lock().unwrap().admin_asked,
            0,
            "no administrator's route"
        );
        seen.lock().unwrap().forbidden = true;
        assert_eq!(
            said("slskd", &settings).await,
            wrong(
                "slskd won't let this key read how it is (403 Forbidden); Melete's key needs slskd's ReadWrite role."
            )
        );
    }

    #[tokio::test]
    async fn libretranslate_must_answer_and_take_the_key() {
        assert_eq!(
            said("translation", &config(&[])).await,
            wrong("There's no address to test yet.")
        );
        let (base, log) = crate::libretranslate::testing::start(true).await;
        let with_key = |key: &str| {
            config(&[
                ("MELETE_LIBRETRANSLATE_URL", base.clone()),
                ("MELETE_LIBRETRANSLATE_API_KEY", key.to_owned()),
            ])
        };
        assert!(!said("translation", &with_key("wrong")).await.ok);
        assert_eq!(
            said(
                "translation",
                &with_key(crate::libretranslate::testing::KEY)
            )
            .await,
            ok("LibreTranslate answered, translating into 2 languages.")
        );
        assert_eq!(
            log.lock().unwrap().last().unwrap()["q"],
            serde_json::json!(["Hello"])
        );
        let gone = config(&[("MELETE_LIBRETRANSLATE_URL", "http://127.0.0.1:9".to_owned())]);
        assert!(!said("translation", &gone).await.ok);
    }

    /// What the settings page offers a Test for is exactly what the Test tries to reach.
    #[tokio::test]
    async fn only_services_with_something_to_reach_are_testable() {
        let none = config(&[]);
        for service in crate::settings::SERVICES {
            let own = said(service.key, &none).await.message == NOTHING_TO_CONNECT_TO;
            assert_eq!(connects(service.key), !own, "{}", service.key);
        }
        assert!(!connects("auto_dj") && connects("tmdb"));
    }

    #[tokio::test]
    async fn flaresolverr_says_which_solver_answers() {
        use crate::flaresolverr::testing::{Plays, start_as};
        let at = |url: String| config(&[("MELETE_FLARESOLVERR_URL", url)]);
        let (flaresolverr, _) = start_as(Plays::FlareSolverr(Some("3.3.21")), "").await;
        assert_eq!(
            said("flaresolverr", &at(flaresolverr)).await,
            ok("FlareSolverr 3.3.21 is ready.")
        );
        let (byparr, _) = start_as(Plays::Byparr, "").await;
        assert_eq!(
            said("flaresolverr", &at(byparr)).await,
            ok(
                "A FlareSolverr-compatible solver answered. It doesn't list its sessions, so whether it gets past a check shows on the first search through it."
            )
        );
    }

    #[tokio::test]
    async fn deluge_flaresolverr_and_the_seedbox_are_tried_as_set() {
        let (deluge, _) = crate::deluge::testing::start().await;
        let downloads = |password: &str| {
            vec![
                ("MELETE_DOWNLOADS", "on".to_owned()),
                ("MELETE_DELUGE_URL", deluge.clone()),
                ("MELETE_DELUGE_PASSWORD", password.to_owned()),
            ]
        };
        assert!(said("downloads", &config(&downloads("pw"))).await.ok);
        let refused = said("downloads", &config(&downloads("nope"))).await;
        assert!(
            !refused.ok && refused.message.starts_with("Deluge: "),
            "{refused:?}"
        );

        let (flaresolverr, _) = crate::flaresolverr::testing::start("").await;
        assert_eq!(
            said(
                "flaresolverr",
                &config(&[("MELETE_FLARESOLVERR_URL", flaresolverr)])
            )
            .await,
            ok("FlareSolverr is ready.")
        );
        let gone = config(&[("MELETE_FLARESOLVERR_URL", "http://127.0.0.1:9".into())]);
        assert!(!said("flaresolverr", &gone).await.ok);

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Completed")).unwrap();
        let server = crate::seedbox::testing::start(root.path()).await;
        let seedbox = |host_key: &str| {
            let mut set = downloads("pw");
            set.extend([
                ("MELETE_SEEDBOX_HOST", "127.0.0.1".to_owned()),
                ("MELETE_SEEDBOX_PORT", server.port.to_string()),
                ("MELETE_SEEDBOX_USER", "user".to_owned()),
                ("MELETE_SEEDBOX_PASSWORD", "pw".to_owned()),
                ("MELETE_SEEDBOX_HOST_KEY", host_key.to_owned()),
                (
                    "MELETE_SEEDBOX_PATHS",
                    "/home/user/Completed=/Completed".to_owned(),
                ),
            ]);
            config(&set)
        };
        assert_eq!(
            said("seedbox", &seedbox(&server.fingerprint)).await,
            ok("Connected, and the host key matched. /Completed has 0 entries.")
        );
        let other = said(
            "seedbox",
            &seedbox("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        )
        .await;
        assert!(
            !other.ok && other.message.starts_with("The seedbox: "),
            "{other:?}"
        );
    }

    #[tokio::test]
    async fn openrouter_says_what_to_set_before_anything_is_tried() {
        assert_eq!(
            said("openrouter", &config(&[])).await,
            wrong("There's no key to test yet.")
        );
    }

    #[tokio::test]
    async fn opensubtitles_says_what_to_set_before_anything_is_tried() {
        assert_eq!(
            said("opensubtitles", &config(&[])).await,
            wrong("There's no API key to test yet.")
        );
    }
}
