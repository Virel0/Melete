//! Melete's outside services, rebuilt in place when their settings change:
//! TMDB, MusicBrainz, slskd, Deluge and the seedbox, FlareSolverr and lyrics, the background
//! work that needs them, and the API that's handed them. A change builds the new clients first
//! (keeping things as they are when one can't be made), then stops the old background work,
//! starts the new and swaps the API over. Requests under way finish with what they began with.
//! The background work stopped is the kind a restart already stops, and each takes up where it
//! left off, as after a restart. What doesn't hang on a service runs throughout: scans,
//! transcodes, the web page, sign-in. What the API keeps in memory (sign-in lockouts, Quick
//! Connect's codes, sign-ins under way at an identity provider, AI search counts) is made once
//! and handed to each API built, so a save keeps it.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use sqlx::SqlitePool;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tower::ServiceExt;

use crate::config::Config;
use crate::ways_in::WaysIn;
use crate::{
    ai, album_fix, api, coverart, deluge, downloads, flaresolverr, follow, fulfil, libretranslate,
    lists, lrclib, musicbrainz, oidc, playlist_import, rss, seedbox, slskd, soulseek_downloads,
    soulseek_import, sound, tmdb, torznab, transcode, transfer,
};

/// Asks for the services to be rebuilt as their settings now stand; answers once they are, or
/// why they couldn't be.
pub type Reload = mpsc::Sender<oneshot::Sender<anyhow::Result<()>>>;

/// What stays the same whatever the services' settings.
pub struct Fixed {
    /// The database.
    pub pool: SqlitePool,
    /// Where `melete.db` and Melete's own files live.
    pub data_dir: PathBuf,
    /// The ffprobe binary.
    pub ffprobe: PathBuf,
    /// The ffmpeg binary.
    pub ffmpeg: PathBuf,
    /// The transcodes running, which outlast a rebuild.
    pub transcoder: Arc<transcode::Transcoder>,
    /// Which parts of Melete each way in opens.
    pub ways_in: Arc<WaysIn>,
    /// TMDB for the work that runs throughout (scans' matching), as it's set up now.
    pub tmdb: watch::Sender<Option<Arc<tmdb::Client>>>,
    /// Where tracks' parts are kept, when the stem separator is set up.
    pub stems_dir: Option<PathBuf>,
}

/// The clients a configuration makes, before anything's started with them.
struct Built {
    tmdb: Option<Arc<tmdb::Client>>,
    flaresolverr: Option<flaresolverr::Client>,
    coverart: Option<Arc<coverart::Client>>,
    fixes: Arc<album_fix::Fixes>,
    slskd: Option<Arc<slskd::Client>>,
    slskd_downloads: Option<PathBuf>,
    pipeline: Option<Arc<downloads::Pipeline>>,
    musicbrainz: Option<Arc<musicbrainz::Client>>,
    /// LRCLIB for the apps' lyrics, on with lyrics for imports.
    lyrics: Option<Arc<lrclib::Client>>,
    /// LibreTranslate for translating lyrics, when it's set up.
    translator: Option<Arc<libretranslate::Client>>,
    song_albums: bool,
    oidc: Option<Arc<oidc::Client>>,
    ai: Option<Arc<ai::Client>>,
}

fn build(fixed: &Fixed, kept: &api::Kept, config: &Config) -> anyhow::Result<Built> {
    let tmdb = match &config.tmdb_token {
        Some(token) => Some(Arc::new(
            tmdb::Client::new(token.expose().to_owned())?
                .with_language(config.metadata_language.clone()),
        )),
        None => None,
    };
    let flaresolverr = config
        .flaresolverr
        .as_deref()
        .map(flaresolverr::Client::new)
        .transpose()?;
    // Album covers from the Cover Art Archive come with MusicBrainz matching, and only then.
    let coverart = match config.musicbrainz {
        true => Some(Arc::new(coverart::Client::new()?)),
        false => None,
    };
    // What every imported album gets, whichever source it came from.
    let fixes = Arc::new(album_fix::Fixes {
        tools: sound::Tools {
            ffmpeg: fixed.ffmpeg.clone(),
            ffprobe: fixed.ffprobe.clone(),
        },
        coverart: coverart.clone(),
        lyrics: match config.lyrics {
            true => Some(lrclib::Client::new()?),
            false => None,
        },
    });
    let (slskd, slskd_downloads) = match &config.slskd {
        Some(settings) => (
            Some(Arc::new(slskd::Client::new(
                &settings.url,
                settings.api_key.clone(),
            )?)),
            settings.downloads.clone(),
        ),
        None => (None, None),
    };
    let pipeline = match &config.downloads {
        Some(settings) => Some(Arc::new(downloads::Pipeline {
            deluge: deluge::Client::new(&settings.deluge_url, settings.deluge_password.clone())?,
            fetcher: downloads::Fetcher::new()?,
            seedbox: settings.seedbox.clone().map(seedbox::Seedbox::new),
        })),
        None => None,
    };
    // One client for the background matching and everything else, so together they keep to
    // MusicBrainz's one request a second.
    let musicbrainz = match config.musicbrainz {
        true => Some(Arc::new(musicbrainz::Client::new()?)),
        false => None,
    };
    let oidc = match &config.oidc {
        Some(settings) => Some(Arc::new(oidc::Client::with_pending(
            settings.clone(),
            kept.oidc.clone(),
        )?)),
        None => None,
    };
    let ai = match &config.openrouter {
        Some(settings) => Some(Arc::new(ai::Client::with_budget(
            settings,
            kept.ai_budget.clone(),
        )?)),
        None => None,
    };
    Ok(Built {
        ai,
        oidc,
        tmdb,
        flaresolverr,
        coverart,
        fixes,
        slskd,
        slskd_downloads,
        pipeline,
        musicbrainz,
        lyrics: match config.lyrics {
            true => Some(Arc::new(lrclib::Client::new()?)),
            false => None,
        },
        translator: match &config.translation {
            Some(t) => Some(Arc::new(libretranslate::Client::new(
                &t.url,
                t.api_key.clone(),
            )?)),
            None => None,
        },
        song_albums: config.song_albums,
    })
}

/// Starts the background work the services need, saying what's on and off.
fn spawn(fixed: &Fixed, built: &Built) -> Vec<JoinHandle<()>> {
    let pool = &fixed.pool;
    let mut tasks = Vec::new();
    if built.tmdb.is_none() {
        tracing::warn!("MELETE_TMDB_TOKEN is not set; skipping metadata");
    }
    if built.flaresolverr.is_some() {
        tracing::info!("FlareSolverr is set; Uindex can be searched");
    }
    if built.fixes.lyrics.is_some() {
        tracing::info!(
            "imported albums, and tracks without lyrics of their own, get lyrics from LRCLIB"
        );
    }
    if let Some(client) = &built.slskd {
        tracing::info!("album searches also ask Soulseek through slskd");
        tasks.push(tokio::spawn(soulseek_downloads::follow(
            pool.clone(),
            client.clone(),
        )));
        match &built.slskd_downloads {
            Some(folder) => tasks.push(tokio::spawn(soulseek_import::work(
                pool.clone(),
                folder.clone(),
                built.fixes.clone(),
            ))),
            None => tracing::warn!(
                "MELETE_SLSKD_DOWNLOADS is not set; finished Soulseek downloads stay in slskd's folder"
            ),
        }
    }
    match &built.pipeline {
        Some(pipeline) => {
            if pipeline.seedbox.is_none() {
                tracing::warn!(
                    "MELETE_SEEDBOX_HOST is not set; finished downloads stay on the seedbox"
                );
            }
            tasks.push(tokio::spawn(downloads::follow(
                pool.clone(),
                pipeline.clone(),
            )));
            tasks.push(tokio::spawn(transfer::work_with(
                pool.clone(),
                pipeline.clone(),
                built.fixes.clone(),
                built.tmdb.clone(),
            )));
        }
        None => tracing::info!("downloads are off; MELETE_DOWNLOADS=on turns them on"),
    }
    tasks.push(tokio::spawn(rss::run(
        pool.clone(),
        built.tmdb.clone(),
        built.pipeline.clone(),
    )));
    if let Some(sources) = api::list_sources(built.tmdb.clone()) {
        tasks.push(tokio::spawn(lists::run(pool.clone(), sources)));
    }
    let sources = fulfil::Sources {
        tmdb: built.tmdb.clone(),
        slskd: built.slskd.clone(),
        pipeline: built.pipeline.clone(),
    };
    match &built.musicbrainz {
        Some(mb) => {
            tasks.push(tokio::spawn(crate::background::match_music(
                pool.clone(),
                mb.clone(),
            )));
            // Followed artists' new albums, through the same client and its pace.
            tasks.push(tokio::spawn(follow::run(pool.clone(), mb.clone())));
            tasks.push(tokio::spawn(playlist_import::work(
                pool.clone(),
                mb.clone(),
                sources.clone(),
            )));
        }
        None => tracing::info!(
            "music isn't matched to MusicBrainz, nor playlists imported, nor followed artists looked up; MELETE_MUSICBRAINZ=on turns it on"
        ),
    }
    if built.song_albums {
        tracing::info!("a requested song's album is wanted once the song is in");
        tasks.push(tokio::spawn(fulfil::albums_after_songs(
            pool.clone(),
            sources,
        )));
    }
    tasks
}

/// The API, handed the services.
fn router(
    fixed: &Fixed,
    kept: &api::Kept,
    built: &Built,
    reload: &Reload,
) -> anyhow::Result<Router> {
    Ok(api::server_with(
        fixed.pool.clone(),
        &fixed.data_dir,
        api::Outside {
            images: tmdb::Images::new()?,
            tmdb: built.tmdb.clone(),
            coverart: built.coverart.clone(),
            musicbrainz: built.musicbrainz.clone(),
            slskd: built.slskd.clone(),
            stems: fixed.stems_dir.clone(),
            lyrics: built.lyrics.clone(),
            translator: built.translator.clone(),
            oidc: built.oidc.clone(),
            ai: built.ai.clone(),
        },
        fixed.ffprobe.clone(),
        fixed.transcoder.clone(),
        built.pipeline.clone(),
        reload.clone(),
        kept,
    )
    .layer(axum::middleware::from_fn_with_state(
        fixed.ways_in.clone(),
        crate::ways_in::guard,
    )))
}

/// The API as the services' settings now stand, rebuilt when they change, and the background
/// work that goes with it.
pub struct Live {
    fixed: Fixed,
    /// Sign-in lockouts and sign-ins under way, Quick Connect's codes and AI search counts:
    /// made once here and handed to every API built, so setting up afresh keeps them.
    kept: api::Kept,
    reload: Reload,
    api: std::sync::RwLock<Router>,
    /// The background work running now; held while the services are rebuilt, so one rebuild
    /// waits for another.
    tasks: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
}

impl Live {
    /// Starts the services as `config` sets them up, and the work that rebuilds them when
    /// asked.
    pub fn start(fixed: Fixed, config: &Config) -> anyhow::Result<Arc<Live>> {
        let (reload, mut asked) = mpsc::channel(4);
        let kept = api::Kept::default();
        let built = build(&fixed, &kept, config)?;
        let api = router(&fixed, &kept, &built, &reload)?;
        let tasks = spawn(&fixed, &built);
        Self::switch(&fixed, &built);
        let live = Arc::new(Live {
            fixed,
            kept,
            reload,
            api: std::sync::RwLock::new(api),
            tasks: tokio::sync::Mutex::new(tasks),
        });
        let rebuilding = Arc::downgrade(&live);
        tokio::spawn(async move {
            while let Some(answer) = asked.recv().await {
                let Some(live) = rebuilding.upgrade() else {
                    return;
                };
                let _ = answer.send(live.rebuild().await);
            }
        });
        Ok(live)
    }

    /// What's set for everyone at once: FlareSolverr for indexers, TMDB for scans.
    fn switch(fixed: &Fixed, built: &Built) {
        torznab::use_flaresolverr(built.flaresolverr.clone());
        fixed.tmdb.send_replace(built.tmdb.clone());
    }

    /// Rebuilds the services as their settings now stand (read once the last rebuild is done,
    /// so two saves at once both count); as they were when the clients can't be made.
    pub async fn rebuild(&self) -> anyhow::Result<()> {
        let mut tasks = self.tasks.lock().await;
        let config = crate::settings::load(&self.fixed.pool).await?;
        let built = build(&self.fixed, &self.kept, &config)?;
        let api = router(&self.fixed, &self.kept, &built, &self.reload)?;
        for task in tasks.iter() {
            task.abort();
        }
        // Stopped before the new work starts, so the two never overlap.
        for task in tasks.drain(..) {
            let _ = task.await;
        }
        *tasks = spawn(&self.fixed, &built);
        Self::switch(&self.fixed, &built);
        *self.api.write().unwrap_or_else(|e| e.into_inner()) = api;
        tracing::info!("outside services set up afresh");
        Ok(())
    }

    /// The API as it stands, for each request to be handed to.
    pub fn service(self: &Arc<Self>) -> Router {
        let live = self.clone();
        Router::new().fallback_service(tower::service_fn(move |request: Request| {
            let api = live.api.read().unwrap_or_else(|e| e.into_inner()).clone();
            async move { api.oneshot(request).await }
        }))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};

    use super::*;
    use crate::db;

    async fn health(live: &Arc<Live>) -> StatusCode {
        let request = HttpRequest::get("/api/v1/health")
            .body(Body::empty())
            .unwrap();
        live.service().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn the_services_are_set_up_afresh_in_place() {
        let pool = db::open_in_memory().await;
        // Nothing here may reach MusicBrainz, which the stored default turns on.
        sqlx::query("UPDATE settings SET value = 'off' WHERE name = 'MELETE_MUSICBRAINZ'")
            .execute(&pool)
            .await
            .unwrap();
        let started = crate::settings::load(&pool).await.unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let (tmdb_now, tmdb) = watch::channel(None);
        let live = Live::start(
            Fixed {
                pool,
                data_dir: scratch.path().to_owned(),
                ffprobe: "ffprobe".into(),
                ffmpeg: "ffmpeg".into(),
                transcoder: transcode::Transcoder::new(
                    "ffmpeg".into(),
                    transcode::Hardware::Software,
                    scratch.path().join("transcode"),
                    1,
                ),
                ways_in: Arc::new(WaysIn::default()),
                tmdb: tmdb_now,
                stems_dir: None,
            },
            &started,
        )
        .unwrap();
        assert!(tmdb.borrow().is_none());
        assert_eq!(health(&live).await, StatusCode::OK);
        let before = live.tasks.lock().await.len();

        // Asked the way a saved setting asks: TMDB is on, for scans too.
        sqlx::query("INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TMDB_TOKEN', 'a-token', 0)")
            .execute(&live.fixed.pool)
            .await
            .unwrap();
        let (answer, answered) = oneshot::channel();
        live.reload.send(answer).await.unwrap();
        answered.await.unwrap().unwrap();
        assert!(tmdb.borrow().is_some());
        assert_eq!(
            health(&live).await,
            StatusCode::OK,
            "the API answers throughout"
        );
        assert_eq!(
            live.tasks.lock().await.len(),
            before + 1,
            "import lists start with TMDB"
        );

        sqlx::query("DELETE FROM settings WHERE name = 'MELETE_TMDB_TOKEN'")
            .execute(&live.fixed.pool)
            .await
            .unwrap();
        live.rebuild().await.unwrap();
        assert!(tmdb.borrow().is_none());
        assert_eq!(live.tasks.lock().await.len(), before, "and stop without it");
    }

    async fn post(live: &Arc<Live>, uri: &str, body: serde_json::Value) -> StatusCode {
        let request = HttpRequest::post(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        live.service().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn setting_up_afresh_keeps_lockouts_and_codes_waiting() {
        let pool = db::open_in_memory().await;
        sqlx::query("UPDATE settings SET value = 'off' WHERE name = 'MELETE_MUSICBRAINZ'")
            .execute(&pool)
            .await
            .unwrap();
        crate::auth::create_user(&pool, "alice", "the-right-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let config = crate::settings::load(&pool).await.unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let live = Live::start(
            Fixed {
                pool,
                data_dir: scratch.path().to_owned(),
                ffprobe: "ffprobe".into(),
                ffmpeg: "ffmpeg".into(),
                transcoder: transcode::Transcoder::new(
                    "ffmpeg".into(),
                    transcode::Hardware::Software,
                    scratch.path().join("transcode"),
                    1,
                ),
                ways_in: Arc::new(WaysIn::default()),
                tmdb: watch::channel(None).0,
                stems_dir: None,
            },
            &config,
        )
        .unwrap();
        let login = |password: &str| {
            serde_json::json!({ "username": "alice", "password": password,
                                "device_name": "Living room iPad" })
        };
        for _ in 0..5 {
            let status = post(&live, "/api/v1/auth/login", login("a-wrong-guess")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let request = HttpRequest::post("/api/v1/auth/quick-connect")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"device_name":"Living room TV"}"#))
            .unwrap();
        let started = live.service().oneshot(request).await.unwrap();
        assert_eq!(started.status(), StatusCode::CREATED);
        let started: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(started.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();

        // An admin saves outside services' settings: the API is built afresh.
        live.rebuild().await.unwrap();

        assert_eq!(
            post(&live, "/api/v1/auth/login", login("the-right-password")).await,
            StatusCode::TOO_MANY_REQUESTS,
            "still locked out after the rebuild"
        );
        let poll = serde_json::json!({ "secret": started["secret"] });
        assert_eq!(
            post(&live, "/api/v1/auth/quick-connect/poll", poll).await,
            StatusCode::ACCEPTED,
            "the code still waits for approval"
        );
    }
}
