//! The `melete` binary: a command given on the command line (see `cli`), or else the server:
//! the config and database, the background work, and the API until Ctrl+C or SIGTERM.

// Outside tests, a function over 100 lines (clippy.toml) or a bare `unwrap` fails the build:
// split long functions into named steps, and say with `expect` why a value can't be missing.
#![deny(clippy::too_many_lines, clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::too_many_lines, clippy::unwrap_used))]

mod access;
mod admin_health;
mod ai;
mod album_fix;
mod album_search;
mod aoide;
mod api;
mod arr;
mod artist_search;
mod auth;
mod backdrop_focus;
mod background;
mod backup;
mod blocklist;
mod calendar;
mod classical;
mod cli;
mod config;
mod cover_colours;
mod coverart;
mod db;
mod deluge;
mod diagnostics;
mod discover_posters;
mod disk;
mod downloads;
mod flaresolverr;
mod follow;
mod fulfil;
mod history;
mod imdb;
mod import;
mod indexers;
mod jobs;
mod languages;
mod library;
mod libretranslate;
mod listen;
mod lists;
mod live;
mod lrclib;
mod lyrics;
mod manage;
mod metadata;
mod mixes;
mod motion;
mod music_convert;
mod music_match;
mod music_profiles;
mod music_release;
mod music_requests;
mod music_spatial;
mod musicbrainz;
mod offline;
mod oidc;
mod opensubtitles;
mod playlist_import;
mod plays;
mod probe;
mod profiles;
mod progress;
mod quick_connect;
mod ratings;
mod release;
mod request_search;
mod requests;
mod romanise;
mod rottentomatoes;
mod rss;
mod search;
mod search_keys;
mod search_missing;
mod seedbox;
mod segments;
mod service_check;
mod settings;
mod slskd;
mod song_search;
mod soulseek;
mod soulseek_downloads;
mod soulseek_import;
mod sound;
mod stems;
mod subtitles;
mod tasks;
mod tmdb;
mod torznab;
mod track_artists;
mod track_picks;
mod transcode;
mod transfer;
mod trickplay;
mod upgrades;
mod want;
mod wanted_edit;
mod wanted_removed;
mod ways_in;

use std::sync::Arc;

use anyhow::{Context, bail};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if show_seedbox_host_key(&args).await? {
        return Ok(());
    }

    if cli::restore_command(&args).await? {
        return Ok(());
    }
    // The environment says where the database is; what the web UI set is in it. The server
    // holds the data folder's lock while it runs, so a restore can't replace its database.
    let data_dir = config::Config::data_dir_from_env();
    let _lock = if args.is_empty() {
        Some(backup::restore::lock(&data_dir)?)
    } else {
        None
    };
    let pool = db::open(&data_dir).await?;
    if !args.is_empty() {
        return cli::run_command(&pool, &args).await;
    }
    let config = settings::load(&pool).await?;
    // Artist searches a restart cut off would otherwise show as running for ever. Not for a
    // command run beside the server, whose searches are still running.
    artist_search::recover(&pool).await?;
    search_missing::recover(&pool).await?;
    library::progress::recover(&pool).await?;
    // Listens from an Aoide import, or pushed before this version, into the play history.
    plays::catch_up_or_log(&pool).await;

    warn_without_users(&pool).await?;
    let libraries = library::sync_config(&pool, &config.libraries).await?;
    library::sync_categories(&pool, &config.libraries, &config.library_categories).await?;
    if libraries.is_empty() {
        tracing::warn!("no libraries configured; set MELETE_LIBRARY_<NAME>=<kind>:<path>");
    }
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    tracing::info!(addr = %config.bind, version = env!("CARGO_PKG_VERSION"), "listening");

    let transcoder = start_transcoder(&config)?;
    log_ways_in(&config.ways_in);
    start_stems(&pool, &config, &transcoder);
    start_motion_covers(&pool, &config, &transcoder);
    // Copies are made behind live conversions; the setting is read as each is due.
    tokio::spawn(offline::work(
        pool.clone(),
        offline::Tools {
            transcoder: transcoder.clone(),
            root: offline::folder(&config.transcode_dir),
            subtitle_cache: config.data_dir.join("subtitles"),
        },
    ));
    // The outside services, rebuilt in place when their settings change.
    let (tmdb_now, tmdb) = tokio::sync::watch::channel(None);
    let live = live::Live::start(
        live::Fixed {
            pool: pool.clone(),
            data_dir: config.data_dir.clone(),
            ffprobe: config.ffprobe.clone(),
            ffmpeg: config.ffmpeg.clone(),
            transcoder,
            ways_in: Arc::new(config.ways_in.clone()),
            tmdb: tmdb_now,
            stems_dir: config.stems.as_ref().map(|s| s.dir.clone()),
        },
        &config,
    )?;
    tokio::spawn(library::watch::run(pool.clone(), libraries.clone()));
    spawn_background_jobs(&pool, &config, libraries, tmdb);

    // Connection info gives sign-in lockouts and the ways in the caller's address.
    let app = live
        .service()
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Runs `melete seedbox host-key <host> [port]` when that's the command, saying whether it was.
async fn show_seedbox_host_key(args: &[String]) -> anyhow::Result<bool> {
    // Needs no configuration: it's how the seedbox's key is found before it's pinned.
    if let ["seedbox", "host-key", host, rest @ ..] = args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        let port = match rest {
            [] => 22,
            [port] => port
                .parse()
                .with_context(|| format!("{port:?} isn't a port"))?,
            _ => bail!("{}", cli::USAGE),
        };
        let key = seedbox::host_key(host, port).await?;
        println!("{host}:{port} presents the host key {key}");
        println!("Check it with your seedbox provider, then set MELETE_SEEDBOX_HOST_KEY={key}");
        return Ok(true);
    }
    Ok(false)
}

/// Warns when there's no user yet, saying how to create the first one.
async fn warn_without_users(pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await?;
    if users == 0 {
        tracing::warn!(
            "no users yet; create one with: docker exec melete melete user add <name> --admin"
        );
    }
    Ok(())
}

/// Checks the transcode folder is apart from the data and the libraries, then starts the
/// transcoder and its supervisor.
fn start_transcoder(config: &config::Config) -> anyhow::Result<Arc<transcode::Transcoder>> {
    let mut protected: Vec<&std::path::Path> = vec![&config.data_dir];
    protected.extend(config.libraries.iter().map(|l| l.path.as_path()));
    transcode::check_dir(&config.transcode_dir, &protected)?;
    let transcoder = transcode::Transcoder::new(
        config.ffmpeg.clone(),
        config.hwaccel,
        config.transcode_dir.clone(),
        config.max_transcodes,
    );
    transcoder.spawn_supervisor();
    Ok(transcoder)
}

/// Says which ways in reach Melete, warning about ones that look like an untrusted proxy.
fn log_ways_in(ways_in: &ways_in::WaysIn) {
    match &ways_in.ways {
        Some(ways) => {
            let names: Vec<&str> = ways.iter().map(|w| w.name.as_str()).collect();
            tracing::info!(ways = ?names, "only the ways in listed reach Melete");
            for way in ways {
                if let Some(max) = way.max_bitrate {
                    tracing::info!(
                        way = way.name,
                        max,
                        "video through this way is held to max bits per second"
                    );
                }
            }
            // A proxy not trusted to say who its clients are makes them all one address: one
            // client's failed sign-ins or Quick Connect codes then hold up all the others.
            for (way, address) in ways_in.untrusted_single_hosts() {
                tracing::warn!(
                    way,
                    %address,
                    "this way in comes from one address, likely a reverse proxy; add it to MELETE_TRUSTED_PROXIES, or every client through it counts as that one address for sign-in limits and Quick Connect"
                );
            }
        }
        None => tracing::info!("MELETE_WAYS_IN is not set; every address reaches everything"),
    }
}

/// Music split into its parts by the separator beside Melete, when it's set up.
fn start_stems(
    pool: &sqlx::SqlitePool,
    config: &config::Config,
    transcoder: &Arc<transcode::Transcoder>,
) {
    match &config.stems {
        Some(folders) => {
            tracing::info!("music is split into its parts by the stem separator");
            tokio::spawn(stems::work(
                pool.clone(),
                folders.clone(),
                transcoder.clone(),
            ));
        }
        None => tracing::info!(
            "music isn't split into parts; MELETE_STEMS_DIR and MELETE_STEMS_JOBS turn it on"
        ),
    }
}

/// Motion covers made from still covers by the renderer beside Melete, when it's set up.
fn start_motion_covers(
    pool: &sqlx::SqlitePool,
    config: &config::Config,
    transcoder: &Arc<transcode::Transcoder>,
) {
    match &config.motion_jobs {
        Some(jobs) => {
            tracing::info!("motion covers are made by the motion cover renderer");
            tokio::spawn(motion::work(
                pool.clone(),
                motion::Folders {
                    jobs: jobs.clone(),
                    videos: motion::videos_dir(&config.data_dir),
                    covers: config.data_dir.join("artwork").join("music"),
                },
                transcoder.clone(),
            ));
        }
        None => tracing::info!("no motion covers are made; MELETE_MOTION_JOBS turns it on"),
    }
}

/// Starts the jobs that run beside the server: analysing music, working out works, the daily
/// backup, reading lyrics, finding segments, colouring covers, and the scans and other
/// housekeeping.
fn spawn_background_jobs(
    pool: &sqlx::SqlitePool,
    config: &config::Config,
    libraries: Vec<library::Library>,
    tmdb: tokio::sync::watch::Receiver<Option<Arc<tmdb::Client>>>,
) {
    tokio::spawn(background::analyse_music(
        pool.clone(),
        sound::Tools {
            ffmpeg: config.ffmpeg.clone(),
            ffprobe: config.ffprobe.clone(),
        },
    ));
    tokio::spawn(background::work_out_works(pool.clone()));
    tokio::spawn(backup::daily(pool.clone(), config.data_dir.clone()));
    tokio::spawn(background::read_lyrics(pool.clone()));
    tokio::spawn(background::find_segments(
        pool.clone(),
        config.ffmpeg.clone(),
    ));
    tokio::spawn(background::colour_covers(
        pool.clone(),
        config.ffmpeg.clone(),
        config.data_dir.join("artwork").join("music"),
    ));
    tokio::spawn(background::housekeeping(
        pool.clone(),
        libraries,
        tmdb,
        config.ffprobe.clone(),
        config.data_dir.join("subtitles"),
        config.data_dir.join("artwork"),
        trickplay::folder(&config.data_dir),
    ));
    tokio::spawn(background::fetch_import_subtitles(
        pool.clone(),
        config.data_dir.join("subtitles"),
    ));
    tokio::spawn(background::make_trickplay(
        pool.clone(),
        config.ffmpeg.clone(),
        trickplay::folder(&config.data_dir),
    ));
}

/// Resolves on Ctrl+C or SIGTERM (what `docker stop` sends to PID 1).
async fn shutdown_signal() {
    // SIGTERM isn't one of the signals that can't be caught, and the runtime has signals on.
    let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM can always be caught");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutting down");
}
