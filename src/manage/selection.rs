//! Changing many titles of a managed list at once, as its selection bar does: monitoring them or
//! stopping, setting their profile, or taking them off the wanted list; and putting titles that
//! are only in the library on the wanted list, monitored, each in the library it's in, as
//! approving an admin's add does (without a request of its own, and without a search: a run
//! for what's missing does that when asked). Each title is changed on its own; one that can't
//! be is left as it is, with why.

use std::collections::HashMap;

use super::*;
use crate::music_requests;
use crate::musicbrainz;
use crate::requests::{self, Choice};
use crate::tmdb;
use crate::wanted_edit::{self, Kind, NotRemoved};

/// Titles changed in one go at most.
pub const MOST: usize = 100;

/// A managed list's service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// Movies.
    Movies,
    /// Series outside anime libraries.
    Series,
    /// Series in anime libraries.
    Anime,
    /// Music, by artist.
    Artists,
}

impl Service {
    /// The service named "movies", "series", "anime" or "artists".
    pub fn parse(name: &str) -> Option<Service> {
        match name {
            "movies" => Some(Service::Movies),
            "series" => Some(Service::Series),
            "anime" => Some(Service::Anime),
            "artists" => Some(Service::Artists),
            _ => None,
        }
    }

    fn kind(self) -> Kind {
        match self {
            Service::Movies => Kind::Movie,
            Service::Series | Service::Anime => Kind::Series,
            Service::Artists => Kind::Artist,
        }
    }
}

/// What's done to the selected titles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Monitored, and those only in the library put on the wanted list with this profile, or
    /// with the defaults' when none.
    Monitor(Option<i64>),
    /// No longer monitored.
    Unmonitor,
    /// Given this profile: a quality profile, or for music a music profile.
    Profile(i64),
    /// Taken off the wanted list by this user; files stay.
    Remove(i64),
}

/// A title left as it was, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unchanged {
    /// Its wanted entry, when it was one.
    pub wanted_id: Option<i64>,
    /// Its folder in the library, when it was only there.
    pub folder: Option<i64>,
    /// Why it wasn't changed.
    pub why: String,
}

/// A title only in the library, now on the wanted list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Wanted {
    /// Its folder in the library.
    pub folder: i64,
    /// Its wanted entry.
    pub wanted_id: i64,
}

/// What changing the selection did.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Outcome {
    /// Titles changed.
    pub changed: usize,
    /// Titles only in the library that are now wanted, each with its wanted entry.
    pub wanted: Vec<Wanted>,
    /// Titles left as they were, with why.
    pub unchanged: Vec<Unchanged>,
}

/// Where TMDB and MusicBrainz are, for titles only in the library.
#[derive(Clone, Copy, Default)]
pub struct Lookups<'a> {
    /// TMDB, for movies and series.
    pub tmdb: Option<&'a tmdb::Client>,
    /// MusicBrainz, for artists.
    pub musicbrainz: Option<&'a musicbrainz::Client>,
}

/// Why a profile can't be given to the service's titles, if it can't.
pub async fn unfit_profile(
    pool: &SqlitePool,
    service: Service,
    id: i64,
) -> anyhow::Result<Option<String>> {
    let fits: Option<bool> = match service {
        Service::Artists => {
            sqlx::query_scalar("SELECT 1 FROM music_profiles WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?
        }
        _ => {
            sqlx::query_scalar("SELECT source = ? FROM quality_profiles WHERE id = ?")
                .bind(if service == Service::Movies {
                    "radarr"
                } else {
                    "sonarr"
                })
                .bind(id)
                .fetch_optional(pool)
                .await?
        }
    };
    Ok((fits != Some(true)).then(|| format!("profile {id} isn't one for this service's titles")))
}

/// Does `action` to each wanted entry, and with Monitor wants each folder; the caller has
/// checked the action fits (a profile for the service, folders only with Monitor).
pub async fn change(
    pool: &SqlitePool,
    lookups: Lookups<'_>,
    service: Service,
    action: Action,
    wanted_ids: &[i64],
    folders: &[i64],
) -> anyhow::Result<Outcome> {
    let mut outcome = Outcome::default();
    for &id in wanted_ids {
        match change_one(pool, service.kind(), action, id).await? {
            None => outcome.changed += 1,
            Some(why) => outcome.unchanged.push(Unchanged {
                wanted_id: Some(id),
                folder: None,
                why,
            }),
        }
    }
    let Action::Monitor(profile) = action else {
        return Ok(outcome);
    };
    if folders.is_empty() {
        return Ok(outcome);
    }
    let found = only_in_library(pool, service).await?;
    for &folder in folders {
        let wanted = match found.get(&folder) {
            None => Err("it isn't a title only in the library".to_owned()),
            Some(title) => want(pool, lookups, service, title, profile).await?,
        };
        match wanted {
            Ok(wanted_id) => {
                outcome.changed += 1;
                outcome.wanted.push(Wanted { folder, wanted_id });
            }
            Err(why) => outcome.unchanged.push(Unchanged {
                wanted_id: None,
                folder: Some(folder),
                why,
            }),
        }
    }
    Ok(outcome)
}

/// Does `action` to one wanted entry; why not, when it couldn't.
async fn change_one(
    pool: &SqlitePool,
    kind: Kind,
    action: Action,
    id: i64,
) -> anyhow::Result<Option<String>> {
    const GONE: &str = "no such wanted entry";
    Ok(match action {
        Action::Monitor(_) | Action::Unmonitor => {
            let on = action != Action::Unmonitor;
            (!wanted_edit::set_monitored(pool, kind, id, on).await?).then(|| GONE.to_owned())
        }
        Action::Profile(profile) => {
            let table = match kind {
                Kind::Movie => "wanted_movies",
                Kind::Series => "wanted_series",
                _ => "wanted_artists",
            };
            let set = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET profile_id = ? WHERE id = ?"
            )))
            .bind(profile)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected();
            (set == 0).then(|| GONE.to_owned())
        }
        Action::Remove(by) => match wanted_edit::remove_by(pool, kind, id, Some(by)).await? {
            Ok(()) => None,
            Err(NotRemoved::NotFound) => Some(GONE.to_owned()),
            Err(NotRemoved::Downloading(n)) => Some(format!(
                "{n} download(s) for it are on their way; remove them under Downloads first"
            )),
        },
    })
}

/// A title only in the library, as its managed list shows it.
struct InLibrary {
    /// The library it's in.
    library: Option<String>,
    /// A movie's or series' TMDB id.
    tmdb_id: Option<i64>,
    /// An artist's MusicBrainz id and name.
    artist: Option<(String, String)>,
}

/// The service's titles only in the library, by folder.
async fn only_in_library(
    pool: &SqlitePool,
    service: Service,
) -> anyhow::Result<HashMap<i64, InLibrary>> {
    // Each row as (folder, TMDB id, MusicBrainz artist id) beside its library and title.
    fn keep<D>(
        titles: Vec<Title<D>>,
        ids: impl Fn(&D) -> (Option<i64>, Option<i64>, Option<String>),
    ) -> HashMap<i64, InLibrary> {
        titles
            .into_iter()
            .filter(|t| !t.wanted)
            .filter_map(|t| {
                let (folder, tmdb_id, mb) = ids(&t.detail);
                let found = InLibrary {
                    library: t.library,
                    tmdb_id,
                    artist: mb.map(|id| (id, t.title)),
                };
                folder.map(|f| (f, found))
            })
            .collect()
    }
    Ok(match service {
        Service::Movies => keep(movies(pool).await?, |d| (d.movie_id, d.tmdb_id, None)),
        Service::Series | Service::Anime => {
            keep(series(pool, service == Service::Anime).await?, |d| {
                (d.series_id, d.tmdb_id, None)
            })
        }
        Service::Artists => keep(artists(pool).await?, |d| {
            (d.artist_id, None, d.mb_artist_id.clone())
        }),
    })
}

/// Puts a title only in the library on the wanted list, monitored, in the library it's in, with
/// `profile` or else the defaults' profile: its wanted id, or why not.
async fn want(
    pool: &SqlitePool,
    lookups: Lookups<'_>,
    service: Service,
    title: &InLibrary,
    profile: Option<i64>,
) -> anyhow::Result<Result<i64, String>> {
    let library_id: Option<i64> = match &title.library {
        Some(name) => {
            sqlx::query_scalar("SELECT id FROM libraries WHERE name = ?")
                .bind(name)
                .fetch_optional(pool)
                .await?
        }
        None => None,
    };
    let music = service == Service::Artists;
    let choice = Choice {
        library_id,
        profile_id: profile.filter(|_| !music),
        music_profile_id: profile.filter(|_| music),
        monitored: Some(true),
        search_now: Some(false),
        seasons: None,
    };
    let wanted = if music {
        let Some((mb_artist_id, name)) = &title.artist else {
            return Ok(Err(
                "its files don't say which MusicBrainz artist it is".into()
            ));
        };
        let asked = music_requests::Asked {
            kind: "artist".into(),
            title: name.clone(),
            artist: None,
            mb_artist_id: Some(mb_artist_id.clone()),
            mb_release_group_id: None,
            mb_recording_id: None,
        };
        music_requests::approve(pool, lookups.musicbrainz, &asked, &choice)
            .await?
            .map(|(_, id)| id)
    } else {
        let Some(tmdb_id) = title.tmdb_id else {
            return Ok(Err(
                "Melete doesn't know which title it is; fix its match first".into(),
            ));
        };
        let kind = if service == Service::Movies {
            "movie"
        } else {
            "series"
        };
        requests::want_from_tmdb(pool, lookups.tmdb, kind, tmdb_id, None, &choice).await?
    };
    let id = match wanted {
        Ok(id) => id,
        Err(refused) => return Ok(Err(refused.to_string())),
    };
    // Wanted already under another folder, perhaps not monitored: it is now.
    wanted_edit::set_monitored(pool, service.kind(), id, true).await?;
    Ok(Ok(id))
}
