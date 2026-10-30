//! The series and anime lists: every wanted series, and every series in the library no wanted
//! series claims, split by whether it's anime.

use std::collections::{BTreeSet, HashMap};

use super::cutoff::EpisodeFile;
use super::*;
use crate::profiles::App;

mod upgrades;

use upgrades::*;

/// A series' own fields in the list.
#[derive(Debug, Clone, Serialize)]
pub struct SeriesDetail {
    /// Its folder in the library, when it's on disk.
    pub series_id: Option<i64>,
    /// TMDB's id, when known.
    pub tmdb_id: Option<i64>,
    /// TheTVDB's id, when known.
    pub tvdb_id: Option<i64>,
    /// IMDb's id, when known.
    pub imdb_id: Option<String>,
    /// standard, daily or anime, as it's wanted; null when it isn't.
    pub series_type: Option<String>,
    /// Episodes on disk: of those monitored and aired when that's known, else all of them.
    pub episodes_on_disk: i64,
    /// Monitored episodes that have aired; null when TMDB's episode list isn't here.
    pub episodes_monitored: Option<i64>,
    /// The monitored episodes below the profile's cutoff, as (season, episode).
    #[serde(skip)]
    pub(crate) unmet_episodes: Vec<(i64, i64)>,
}

impl super::Folder for SeriesDetail {
    fn folder(&self) -> Option<i64> {
        self.series_id
    }
}

#[derive(sqlx::FromRow)]
struct WantedRow {
    id: i64,
    tvdb_id: i64,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
    title: String,
    year: Option<i64>,
    monitored: bool,
    series_type: String,
    added_at: i64,
    series_id: Option<i64>,
    profile_id: Option<i64>,
    profile: Option<String>,
    /// The profile's name as the arr gave it.
    arr_profile: Option<String>,
    original_language: Option<String>,
    library: Option<String>,
    category: Option<String>,
    /// Series or anime, as the managed lists place it.
    service: String,
    poster_path: Option<String>,
    /// Whether TMDB's episode list is here.
    known: bool,
}

#[derive(sqlx::FromRow)]
struct LibraryRow {
    id: i64,
    match_tmdb_id: Option<i64>,
    arr_tmdb_id: Option<i64>,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    arr_tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    title: String,
    year: Option<i64>,
    poster_path: Option<String>,
    /// The folder's own poster, by its artwork name.
    local_poster: Option<String>,
    library: String,
    category: Option<String>,
    size: Option<i64>,
    first: Option<i64>,
}

pub(super) const WANTED: &str = "
    SELECT w.id, w.tvdb_id, w.tmdb_id, w.imdb_id, coalesce(t.name, w.title) AS title,
           coalesce(t.year, w.year) AS year, w.monitored, w.series_type, w.added_at, w.series_id,
           w.profile_id, coalesce(p.name, w.quality_profile) AS profile,
           w.quality_profile AS arr_profile, w.original_language, l.name AS library,
           l.category, t.poster_path,
           CASE WHEN w.series_type = 'anime'
                     OR coalesce((SELECT sl.category FROM series s
                                  JOIN libraries sl ON sl.id = s.library_id
                                  WHERE s.id = w.series_id), l.category) = 'anime'
                THEN 'anime' ELSE 'series' END AS service,
           EXISTS (SELECT 1 FROM tmdb_episodes e WHERE e.series_tmdb_id = t.tmdb_id) AS known
    FROM wanted_series w
    LEFT JOIN libraries l ON l.id = w.library_id
    LEFT JOIN quality_profiles p ON p.id = w.profile_id
    LEFT JOIN tmdb_series t ON t.tmdb_id = w.tmdb_id";

pub(super) const LIBRARY: &str = "
    SELECT s.id, s.match_tmdb_id, s.arr_tmdb_id, s.tmdb_id, s.tvdb_id, s.arr_tvdb_id, s.imdb_id,
           coalesce(t.name, s.parsed_title) AS title, coalesce(t.year, s.parsed_year) AS year,
           t.poster_path, s.local_poster_key AS local_poster, l.name AS library, l.category,
           f.size, f.first
    FROM series s
    JOIN libraries l ON l.id = s.library_id
    LEFT JOIN tmdb_series t ON t.tmdb_id = s.match_tmdb_id
    LEFT JOIN (SELECT series_id, sum(size) AS size, min(coalesce(added_at, modified_at)) AS first
               FROM media_files WHERE series_id IS NOT NULL AND missing_since IS NULL
               GROUP BY series_id) f ON f.series_id = s.id
    WHERE s.missing_since IS NULL";

/// Monitored episodes of monitored wanted series, and whether each has aired: in a season
/// that's monitored, and not left out on its own, as searches and RSS decide.
pub(super) const MONITORED_EPISODES: &str = "
    SELECT w.id, e.season, e.episode, coalesce(e.air_date <= date('now'), 0)
    FROM wanted_series w
    JOIN tmdb_episodes e ON e.series_tmdb_id = w.tmdb_id
    JOIN wanted_seasons ws ON ws.wanted_series_id = w.id AND ws.season = e.season
    LEFT JOIN wanted_episode_overrides o
        ON o.wanted_series_id = w.id AND o.season = e.season AND o.episode = e.episode
    WHERE w.monitored = 1 AND ws.monitored = 1 AND coalesce(o.monitored, 1) = 1";

/// Every present numbered episode file: its series, id, path, season and episodes.
pub(super) const EPISODE_FILES: &str = "
    SELECT series_id, id, rel_path, season, episode, episode_end FROM media_files
    WHERE series_id IS NOT NULL AND missing_since IS NULL AND season IS NOT NULL";

/// One wanted series' monitored episodes: (season, episode) aired, and how many are to come.
#[derive(Default)]
struct Monitored {
    aired: Vec<(i64, i64)>,
    upcoming: i64,
}

/// Every managed series, unsorted, anime or not. A series is anime when it's wanted as anime,
/// or it's in (or going to) a library with the anime category. A wanted series claims the
/// folder it was adopted with, and any folder matched to the same TMDB or TheTVDB series.
pub async fn series(pool: &SqlitePool, anime: bool) -> anyhow::Result<Vec<Title<SeriesDetail>>> {
    let wanted: Vec<WantedRow> = sqlx::query_as(WANTED).fetch_all(pool).await?;
    let library: Vec<LibraryRow> = sqlx::query_as(LIBRARY).fetch_all(pool).await?;
    let mut monitored: HashMap<i64, Monitored> = HashMap::new();
    let rows: Vec<(i64, i64, i64, bool)> =
        sqlx::query_as(MONITORED_EPISODES).fetch_all(pool).await?;
    for (id, season, episode, aired) in rows {
        let entry = monitored.entry(id).or_default();
        if aired {
            entry.aired.push((season, episode));
        } else {
            entry.upcoming += 1;
        }
    }
    let mut files: HashMap<i64, Vec<EpisodeFile>> = HashMap::new();
    let mut on_disk: HashMap<i64, BTreeSet<(i64, i64)>> = HashMap::new();
    let rows: Vec<(i64, i64, String, i64, i64, Option<i64>)> =
        sqlx::query_as(EPISODE_FILES).fetch_all(pool).await?;
    for (series, id, path, season, first, last) in rows {
        let file = EpisodeFile {
            id,
            path,
            season,
            first,
            last: last.unwrap_or(first),
        };
        let episodes = on_disk.entry(series).or_default();
        episodes.extend(file.episodes().map(|e| (season, e)));
        files.entry(series).or_default().push(file);
    }
    let seasons = monitored_seasons(pool).await?;
    let rules = Rules::load(pool).await?;
    let downloads = season_downloads(pool).await?;
    let mut profiles = cutoff::Profiles::new(pool, App::Sonarr).await?;

    let claimed = claim(&wanted, &library);
    let mut titles = Vec::new();
    for (w, folders) in wanted.into_iter().zip(&claimed.wanted) {
        let category = shown(folders).map_or(w.category.as_deref(), |s| s.category.as_deref());
        if (w.series_type == "anime" || category == Some("anime")) != anime {
            continue;
        }
        let mut here = BTreeSet::new();
        for folder in folders {
            here.extend(on_disk.get(&folder.id).into_iter().flatten().copied());
        }
        let linked = w.series_id.and_then(|id| files.get(&id));
        let ctx = Context {
            folders,
            here: &here,
            monitored: monitored.remove(&w.id).unwrap_or_default(),
            seasons: seasons.get(&w.id).map_or(&[][..], Vec::as_slice),
            downloads: downloads.get(&w.id).map_or(&[][..], Vec::as_slice),
            cutoff: series_cutoff(pool, &mut profiles, &w, linked, &rules).await?,
        };
        titles.push(wanted_title(w, ctx));
    }
    for show in claimed.unclaimed {
        let title = library_title(show, on_disk.get(&show.id).map_or(0, |e| e.len()));
        if show.size.is_some() && is_anime(&title) == anime {
            titles.push(title);
        }
    }
    Ok(titles)
}

fn is_anime(title: &Title<SeriesDetail>) -> bool {
    title.detail.series_type.as_deref() == Some("anime")
        || title.category.as_deref() == Some("anime")
}

/// The folder a wanted series' row shows: the first holding a file, else the first claimed.
fn shown<'a>(folders: &[&'a LibraryRow]) -> Option<&'a LibraryRow> {
    folders
        .iter()
        .find(|f| f.size.is_some())
        .or(folders.first())
        .copied()
}

struct Claimed<'a> {
    wanted: Vec<Vec<&'a LibraryRow>>,
    unclaimed: Vec<&'a LibraryRow>,
}

/// Each wanted series' folders, and the folders nobody wants.
fn claim<'a>(wanted: &[WantedRow], library: &'a [LibraryRow]) -> Claimed<'a> {
    let mut by_folder = HashMap::new();
    let mut by_tmdb = HashMap::new();
    let mut by_tvdb = HashMap::new();
    for (i, w) in wanted.iter().enumerate() {
        if let Some(id) = w.series_id {
            by_folder.insert(id, i);
        }
        if let Some(id) = w.tmdb_id {
            by_tmdb.entry(id).or_insert(i);
        }
        by_tvdb.insert(w.tvdb_id, i);
    }
    let mut claimed = Claimed {
        wanted: vec![Vec::new(); wanted.len()],
        unclaimed: Vec::new(),
    };
    for show in library {
        let owner = by_folder.get(&show.id).copied().or_else(|| {
            let tmdb = [show.match_tmdb_id, show.arr_tmdb_id, show.tmdb_id];
            let tvdb = [show.tvdb_id, show.arr_tvdb_id];
            tmdb.into_iter()
                .flatten()
                .find_map(|id| by_tmdb.get(&id).copied())
                .or_else(|| {
                    tvdb.into_iter()
                        .flatten()
                        .find_map(|id| by_tvdb.get(&id).copied())
                })
        });
        match owner {
            Some(i) => claimed.wanted[i].push(show),
            None => claimed.unclaimed.push(show),
        }
    }
    claimed
}

/// Each monitored wanted series' monitored seasons, specials aside.
async fn monitored_seasons(pool: &SqlitePool) -> anyhow::Result<HashMap<i64, Vec<i64>>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT ws.wanted_series_id, ws.season FROM wanted_seasons ws
         JOIN wanted_series w ON w.id = ws.wanted_series_id
         WHERE w.monitored = 1 AND ws.monitored = 1 AND ws.season > 0",
    )
    .fetch_all(pool)
    .await?;
    let mut seasons: HashMap<i64, Vec<i64>> = HashMap::new();
    for (id, season) in rows {
        seasons.entry(id).or_default().push(season);
    }
    Ok(seasons)
}

/// Downloads under way for each wanted series, newest first, with the season each is for
/// (none for a whole-series pack).
async fn season_downloads(
    pool: &SqlitePool,
) -> anyhow::Result<HashMap<i64, Vec<(Option<i64>, DownloadNow)>>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        wanted_series_id: i64,
        id: i64,
        season: Option<i64>,
        state: String,
        progress: f64,
        message: Option<String>,
    }
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT wanted_series_id, id, season, state, progress, message FROM downloads
         WHERE wanted_series_id IS NOT NULL AND state IN {UNDER_WAY} ORDER BY id DESC"
    )))
    .fetch_all(pool)
    .await?;
    let mut downloads: HashMap<i64, Vec<_>> = HashMap::new();
    for row in rows {
        let now = DownloadNow::new(row.id, &row.state, row.progress, row.message.as_deref());
        downloads
            .entry(row.wanted_series_id)
            .or_default()
            .push((row.season, now));
    }
    Ok(downloads)
}

/// What a wanted series' row is worked out from, besides the row itself.
struct Context<'a> {
    folders: &'a [&'a LibraryRow],
    /// Its episodes on disk, in every folder it claims.
    here: &'a BTreeSet<(i64, i64)>,
    monitored: Monitored,
    seasons: &'a [i64],
    downloads: &'a [(Option<i64>, DownloadNow)],
    /// Whether an episode is below the cutoff, and which.
    cutoff: (Option<bool>, Vec<(i64, i64)>),
}

/// A wanted series' row. Missing are its monitored episodes that have aired and aren't on
/// disk; when TMDB's episode list isn't here, each monitored season (specials aside) with
/// nothing of it on disk counts, since what's missing from one partly here can't be told.
/// A download counts as bringing what it lacks when it's for a season with something
/// missing, for the whole series, or when nothing of it is on disk yet.
fn wanted_title(w: WantedRow, ctx: Context) -> Title<SeriesDetail> {
    let Context {
        folders,
        here,
        monitored,
        seasons,
        downloads,
        cutoff: (cutoff_unmet, unmet_episodes),
    } = ctx;
    let mut missing_in: HashMap<i64, i64> = HashMap::new();
    let (on_disk_count, monitored_count) = if w.known {
        for key in monitored.aired.iter().filter(|k| !here.contains(k)) {
            *missing_in.entry(key.0).or_default() += 1;
        }
        let present = monitored.aired.iter().filter(|k| here.contains(k)).count();
        (present as i64, Some(monitored.aired.len() as i64))
    } else {
        for season in seasons
            .iter()
            .filter(|s| !here.iter().any(|(h, _)| h == *s))
        {
            missing_in.insert(*season, 1);
        }
        (here.len() as i64, None)
    };
    let missing: i64 = missing_in.values().sum();
    let on_disk = !here.is_empty();
    let under_way = downloads
        .iter()
        .any(|(season, _)| !on_disk || season.is_none_or(|s| missing_in.contains_key(&s)));
    let facts = Facts {
        on_disk,
        missing,
        under_way,
        // Nothing monitored to come or out: a monitored series with every season off.
        monitored: w.monitored && (missing > 0 || monitored.upcoming > 0 || !seasons.is_empty()),
    };
    let shown = shown(folders);
    // The folder's own poster, else the wanted series' TMDB one, else the folder's.
    let poster_path = shown
        .and_then(|s| s.local_poster.clone())
        .or(w.poster_path)
        .or_else(|| shown.and_then(|s| s.poster_path.clone()));
    Title {
        wanted_id: Some(w.id),
        title: w.title,
        year: w.year,
        poster: poster(poster_path),
        state: facts.state(),
        monitored: w.monitored,
        wanted: true,
        profile_id: w.profile_id,
        profile: w.profile,
        library: shown.map(|s| s.library.clone()).or(w.library),
        category: shown.map_or(w.category, |s| s.category.clone()),
        size: folders.iter().filter_map(|f| f.size).sum(),
        added_at: Some(w.added_at),
        download: downloads.first().map(|(_, d)| d.clone()),
        cutoff_unmet,
        detail: SeriesDetail {
            series_id: shown.map(|s| s.id).or(w.series_id),
            tmdb_id: w.tmdb_id,
            tvdb_id: Some(w.tvdb_id),
            imdb_id: w.imdb_id.or_else(|| shown.and_then(|s| s.imdb_id.clone())),
            series_type: Some(w.series_type),
            episodes_on_disk: on_disk_count,
            episodes_monitored: monitored_count,
            unmet_episodes,
        },
    }
}

/// A series in the library that isn't wanted, with how many episodes of it are on disk.
fn library_title(show: &LibraryRow, episodes: usize) -> Title<SeriesDetail> {
    Title {
        wanted_id: None,
        title: show.title.clone(),
        year: show.year,
        poster: poster(show.local_poster.clone().or(show.poster_path.clone())),
        state: State::InLibrary,
        monitored: false,
        wanted: false,
        profile_id: None,
        profile: None,
        library: Some(show.library.clone()),
        category: show.category.clone(),
        size: show.size.unwrap_or(0),
        added_at: show.first,
        download: None,
        cutoff_unmet: None,
        detail: SeriesDetail {
            series_id: Some(show.id),
            tmdb_id: show.match_tmdb_id.or(show.arr_tmdb_id).or(show.tmdb_id),
            tvdb_id: show.tvdb_id.or(show.arr_tvdb_id),
            imdb_id: show.imdb_id.clone(),
            series_type: None,
            episodes_on_disk: episodes as i64,
            episodes_monitored: None,
            unmet_episodes: Vec::new(),
        },
    }
}
