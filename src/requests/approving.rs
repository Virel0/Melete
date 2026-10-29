//! Approving a request: placing the title where the defaults say or the approver chooses, and
//! wanting it from TMDB.

use super::*;
use crate::wanted_edit::{self, Kind};

/// Approves a request: its title joins the wanted list where `choice` or the defaults say.
pub async fn approve(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    mb: Option<&musicbrainz::Client>,
    id: i64,
    admin: &User,
    choice: &Choice,
) -> anyhow::Result<Result<RequestView, Refused>> {
    if let Err(refused) = approve_as(pool, tmdb, mb, id, Some(admin.id), choice).await? {
        return Ok(Err(refused));
    }
    Ok(Ok(get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("request {id} vanished"))?))
}

#[derive(sqlx::FromRow)]
pub(super) struct Asked {
    pub(super) kind: String,
    pub(super) tmdb_id: Option<i64>,
    pub(super) season: Option<i64>,
    pub(super) state: String,
    #[sqlx(flatten)]
    pub(super) music: music_requests::Asked,
}

/// Where an approved title goes: the approver's choice, else the defaults for its kind, else
/// the only library of that kind and category (anime, or none), else the only one of that kind.
pub(super) async fn place(
    pool: &SqlitePool,
    defaults: &str,
    library_kind: &str,
    choice: &Choice,
) -> anyhow::Result<Result<(i64, Option<i64>), Refused>> {
    let set: Option<(Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT library_id, profile_id FROM request_defaults WHERE kind = ?")
            .bind(defaults)
            .fetch_optional(pool)
            .await?;
    let (set_library, set_profile) = set.unwrap_or_default();
    let library = match choice.library_id.or(set_library) {
        Some(id) => {
            let kind: Option<String> =
                sqlx::query_scalar("SELECT kind FROM libraries WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if kind.as_deref() != Some(library_kind) {
                return Ok(Err(Refused::Invalid(format!(
                    "library {id} isn't a {library_kind} library"
                ))));
            }
            id
        }
        None => {
            let all: Vec<(i64, Option<String>)> =
                sqlx::query_as("SELECT id, category FROM libraries WHERE kind = ? ORDER BY id")
                    .bind(library_kind)
                    .fetch_all(pool)
                    .await?;
            // A library's category settles it: anime goes to the one anime library, the
            // rest to the one library without a category, when there's exactly one.
            let category = (defaults == "anime").then_some("anime");
            let fitting: Vec<i64> = all
                .iter()
                .filter(|(_, c)| c.as_deref() == category)
                .map(|(id, _)| *id)
                .collect();
            let only = if fitting.len() == 1 {
                fitting
            } else {
                all.into_iter().map(|(id, _)| id).collect()
            };
            match only.as_slice() {
                [one] => *one,
                [] => {
                    return Ok(Err(Refused::NoLibrary(format!(
                        "there's no {library_kind} library to put it in"
                    ))));
                }
                _ => {
                    return Ok(Err(Refused::NoLibrary(format!(
                        "there's more than one {library_kind} library; choose one, or set where approved {defaults} go"
                    ))));
                }
            }
        }
    };
    let profile = choice.profile_id.or(set_profile);
    if let Some(id) = profile {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM quality_profiles WHERE id = ?)")
                .bind(id)
                .fetch_one(pool)
                .await?;
        if !exists {
            return Ok(Err(Refused::Invalid(format!(
                "there's no quality profile {id}"
            ))));
        }
    }
    Ok(Ok((library, profile)))
}

pub(super) async fn approve_as(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    mb: Option<&musicbrainz::Client>,
    id: i64,
    by: Option<i64>,
    choice: &Choice,
) -> anyhow::Result<Result<(), Refused>> {
    let asked: Option<Asked> = sqlx::query_as(
        "SELECT kind, tmdb_id, season, state, title, artist, mb_artist_id, mb_release_group_id,
             mb_recording_id
         FROM requests WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(asked) = asked else {
        return Ok(Err(Refused::NotFound));
    };
    if asked.state != "requested" {
        return Ok(Err(Refused::Decided));
    }
    if let Some(why) = choice.misfit(&asked.kind) {
        return Ok(Err(Refused::Invalid(why)));
    }
    let Some(tmdb_id) = asked.tmdb_id else {
        let (column, wanted_id) =
            match music_requests::approve(pool, mb, &asked.music, choice).await? {
                Ok(linked) => linked,
                Err(refused) => return Ok(Err(refused)),
            };
        apply_choice(pool, column, wanted_id, choice).await?;
        return decided(pool, id, column, wanted_id, by).await;
    };
    let wanted_id =
        match want_from_tmdb(pool, tmdb, &asked.kind, tmdb_id, asked.season, choice).await? {
            Ok(id) => id,
            Err(refused) => return Ok(Err(refused)),
        };
    let column = if asked.kind == "movie" {
        "wanted_movie_id"
    } else {
        "wanted_series_id"
    };
    apply_choice(pool, column, wanted_id, choice).await?;
    decided(pool, id, column, wanted_id, by).await
}

/// What an approver chose beyond where it goes, once it's on the wanted list: the seasons
/// monitored, and whether it's monitored at all. Both are choices made in Melete, which imports
/// keep.
async fn apply_choice(
    pool: &SqlitePool,
    column: &str,
    wanted_id: i64,
    choice: &Choice,
) -> anyhow::Result<()> {
    for season in choice.seasons.iter().flatten() {
        wanted_edit::set_season_monitored(pool, wanted_id, *season, true).await?;
    }
    let kind = match column {
        "wanted_movie_id" => Kind::Movie,
        "wanted_series_id" => Kind::Series,
        "wanted_artist_id" => Kind::Artist,
        "wanted_album_id" => Kind::Album,
        _ => return Ok(()),
    };
    if let Some(monitored) = choice.monitored {
        wanted_edit::set_monitored(pool, kind, wanted_id, monitored).await?;
    }
    Ok(())
}

/// Puts a movie or series from TMDB on the wanted list as an approved request does: where
/// approved ones go (or as `choice` says), monitored, a series' `season` or all of them. One
/// wanted already keeps its place, a season asked for monitored there. Its wanted id.
pub(crate) async fn want_from_tmdb(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    kind: &str,
    tmdb_id: i64,
    season: Option<i64>,
    choice: &Choice,
) -> anyhow::Result<Result<i64, Refused>> {
    // Wanted already: it has its place; a season asked for is monitored there.
    let existing: Option<i64> = sqlx::query_scalar(if kind == "movie" {
        "SELECT id FROM wanted_movies WHERE tmdb_id = ?"
    } else {
        "SELECT id FROM wanted_series WHERE tmdb_id = ?"
    })
    .bind(tmdb_id)
    .fetch_optional(pool)
    .await?;
    let wanted = if let Some(id) = existing {
        if kind == "series" {
            want::monitor_asked(pool, id, season.map_or(Seasons::None, Seasons::Only)).await?;
        }
        Wanted::Already(id)
    } else if let (Some(tmdb), "movie") = (tmdb, kind) {
        let (library_id, profile_id) = match place(pool, "movies", "movies", choice).await? {
            Ok(placed) => placed,
            Err(refused) => return Ok(Err(refused)),
        };
        let placement = Placement {
            monitored: true,
            quality_profile: None,
            profile_id,
            library_id,
            seasons: Seasons::None,
            list_id: None,
            // Asked for, so wanted again even if an admin took it off the list.
            keep_removals: false,
        };
        want::movie(pool, tmdb, tmdb_id, &placement).await?
    } else if let Some(tmdb) = tmdb {
        // Anime has defaults of its own, as it often has a library of its own.
        let anime = tmdb
            .series(tmdb_id)
            .await?
            .is_some_and(|s| want::is_anime(&s));
        let defaults = if anime { "anime" } else { "series" };
        let placed = match place(pool, defaults, "shows", choice).await? {
            Ok(placed) => Ok(placed),
            // No anime defaults set: the series ones serve.
            Err(Refused::NoLibrary(_)) if anime => place(pool, "series", "shows", choice).await?,
            Err(refused) => Err(refused),
        };
        let (library_id, profile_id) = match placed {
            Ok(placed) => placed,
            Err(refused) => return Ok(Err(refused)),
        };
        // Seasons the approver listed are monitored once it's on the list.
        let seasons = match (&choice.seasons, season) {
            (Some(_), _) => Seasons::None,
            (None, Some(season)) => Seasons::Only(season),
            (None, None) => Seasons::All,
        };
        let placement = Placement {
            monitored: true,
            quality_profile: None,
            profile_id,
            library_id,
            seasons,
            list_id: None,
            // Asked for, so wanted again even if an admin took it off the list.
            keep_removals: false,
        };
        want::series(pool, tmdb, tmdb_id, &placement).await?
    } else {
        return Ok(Err(Refused::NoMetadata));
    };
    Ok(match wanted {
        Wanted::Added(id) | Wanted::Already(id) => Ok(id),
        Wanted::Skipped(why) => Err(Refused::Unavailable(why)),
    })
}

/// Marks a request approved, linked to what it became on the wanted list.
pub(super) async fn decided(
    pool: &SqlitePool,
    id: i64,
    column: &str,
    wanted_id: i64,
    by: Option<i64>,
) -> anyhow::Result<Result<(), Refused>> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE requests SET state = 'approved', {column} = ?, decided_at = ?, decided_by = ?
         WHERE id = ? AND state = 'requested'"
    )))
    .bind(wanted_id)
    .bind(unix_now())
    .bind(by)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(Ok(()))
}
