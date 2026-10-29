//! What's missing in an area, and whether it still is when its turn comes.

use super::*;

/// One search a run makes.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Item {
    Movie {
        id: i64,
        title: String,
    },
    Season {
        id: i64,
        title: String,
        season: i64,
    },
    /// One episode, searched for an upgrade.
    Episode {
        id: i64,
        title: String,
        season: i64,
        episode: i64,
    },
    Album {
        id: i64,
        title: String,
    },
}

impl Item {
    pub(super) fn name(&self) -> String {
        match self {
            Item::Movie { title, .. } | Item::Album { title, .. } => title.clone(),
            Item::Season { title, season, .. } => format!("{title}, season {season}"),
            Item::Episode {
                title,
                season,
                episode,
                ..
            } => format!("{title}, season {season} episode {episode}"),
        }
    }

    /// The search an item makes, for a movie or series.
    pub(super) fn target(&self) -> Option<Target> {
        match *self {
            Item::Movie { id, .. } => Some(Target::Movie(id)),
            Item::Season { id, season, .. } => Some(Target::Series {
                id,
                season,
                episode: None,
            }),
            Item::Episode {
                id,
                season,
                episode,
                ..
            } => Some(Target::Series {
                id,
                season,
                episode: Some(episode),
            }),
            Item::Album { .. } => None,
        }
    }
}

/// What the run has to do in the area now.
pub(super) async fn to_do(pool: &SqlitePool, area: Area, goal: Goal) -> anyhow::Result<Vec<Item>> {
    match goal {
        Goal::Missing => missing(pool, area).await,
        Goal::CutoffUnmet => below_cutoff(pool, area).await,
    }
}

/// Whether an item still needs its search when its turn comes.
pub(super) async fn still_to_do(
    pool: &SqlitePool,
    goal: Goal,
    item: &Item,
) -> anyhow::Result<bool> {
    match goal {
        Goal::Missing => still_missing(pool, item).await,
        Goal::CutoffUnmet => still_below_cutoff(pool, item).await,
    }
}

/// What's missing in the area now.
pub(super) async fn missing(pool: &SqlitePool, area: Area) -> anyhow::Result<Vec<Item>> {
    let mut items = Vec::new();
    match area {
        Area::Movies => {
            let movies: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, title FROM wanted_movies WHERE monitored = 1 ORDER BY added_at, id",
            )
            .fetch_all(pool)
            .await?;
            for (id, title) in movies {
                if still_missing(
                    pool,
                    &Item::Movie {
                        id,
                        title: title.clone(),
                    },
                )
                .await?
                {
                    items.push(Item::Movie { id, title });
                }
            }
        }
        Area::Series => {
            let series: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, title FROM wanted_series WHERE monitored = 1 ORDER BY added_at, id",
            )
            .fetch_all(pool)
            .await?;
            for (id, title) in series {
                for season in progress::seasons(pool, id).await? {
                    if season_missing(&season) {
                        items.push(Item::Season {
                            id,
                            title: title.clone(),
                            season: season.season,
                        });
                    }
                }
            }
        }
        Area::Albums => {
            for album in album_search::monitored(pool).await? {
                if !album_search::taken(pool, album.id).await? {
                    items.push(Item::Album {
                        id: album.id,
                        title: format!("{} by {}", album.title, album.artist),
                    });
                }
            }
        }
    }
    Ok(items)
}

/// A monitored season still missing episodes with nothing on its way. Specials are left out,
/// as an approved request leaves them; a season whose episodes TMDB hasn't listed counts only
/// while none of it is on disk, since what's missing from it can't be told.
pub(super) fn season_missing(season: &progress::SeasonProgress) -> bool {
    season.season > 0
        && season.step == Step::Searching
        && (season.episodes.is_some() || season.in_library == 0)
}

/// Whether it's still missing, since a sync or a request may have grabbed it meanwhile.
pub(super) async fn still_missing(pool: &SqlitePool, item: &Item) -> anyhow::Result<bool> {
    Ok(match item {
        Item::Movie { id, .. } => {
            progress::of(pool, "movie", *id).await?.map(|p| p.step) == Some(Step::Searching)
        }
        Item::Season { id, season, .. } => progress::seasons(pool, *id)
            .await?
            .iter()
            .any(|s| s.season == *season && season_missing(s)),
        Item::Album { id, .. } => {
            let monitored: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM wanted_albums w JOIN wanted_artists a
                                ON a.id = w.wanted_artist_id
                                WHERE w.id = ? AND w.monitored = 1 AND a.monitored = 1)",
            )
            .bind(id)
            .fetch_one(pool)
            .await?;
            monitored && !album_search::taken(pool, *id).await?
        }
        // Searched for upgrades only.
        Item::Episode { .. } => false,
    })
}
