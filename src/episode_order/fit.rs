//! How well each of a series' groups fits its files, choosing one by itself when exactly one
//! fits exactly, and the closest to suggest otherwise.

use super::*;

/// One of a series' episode groups, as offered to an admin.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Offered {
    /// TMDB's id for the group.
    pub group_id: String,
    pub name: String,
    /// TMDB's type: 1 original air date, 2 absolute, 3 DVD, 4 digital, 5 story arc,
    /// 6 production, 7 TV.
    pub kind: Option<i64>,
    /// Its seasons, specials left out, with how many episodes each has.
    pub seasons: Vec<SeasonCount>,
    /// Whether it has exactly the seasons and episodes the files have.
    pub exact: bool,
    /// How many of the files' episodes it has.
    pub covered: usize,
}

/// A season and how many episodes it has.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeasonCount {
    pub season: i64,
    pub episodes: usize,
}

/// A fetched group's seasons, of the episodes TMDB lists for the series.
struct Group {
    id: String,
    name: String,
    kind: Option<i64>,
    seasons: Seasons,
}

/// A group's id, name and type, with one of its episodes (season, episode) when it has any.
type GroupRow = (String, String, Option<i64>, Option<i64>, Option<i64>);

async fn groups(conn: &mut sqlx::SqliteConnection, tmdb_id: i64) -> sqlx::Result<Vec<Group>> {
    let rows: Vec<GroupRow> = sqlx::query_as(
        "SELECT g.id, g.name, g.kind, x.season, x.episode FROM tmdb_episode_groups g
         LEFT JOIN (SELECT ge.group_id, ge.season, ge.episode FROM tmdb_episode_group_episodes ge
                    JOIN tmdb_episodes e
                        ON e.series_tmdb_id = ?1 AND e.tmdb_id = ge.episode_tmdb_id
                    WHERE ge.season > 0) x ON x.group_id = g.id
         WHERE g.series_tmdb_id = ?1 AND g.fetched_at IS NOT NULL
         ORDER BY g.id",
    )
    .bind(tmdb_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut groups: Vec<Group> = Vec::new();
    for (id, name, kind, season, episode) in rows {
        if groups.last().is_none_or(|g| g.id != id) {
            groups.push(Group {
                id,
                name,
                kind,
                seasons: Seasons::new(),
            });
        }
        if let (Some(season), Some(episode), Some(group)) = (season, episode, groups.last_mut()) {
            group.seasons.entry(season).or_default().insert(episode);
        }
    }
    Ok(groups)
}

fn covered(files: &Seasons, seasons: &Seasons) -> usize {
    files
        .iter()
        .map(|(s, episodes)| {
            seasons
                .get(s)
                .map_or(0, |has| episodes.intersection(has).count())
        })
        .sum()
}

fn total(seasons: &Seasons) -> usize {
    seasons.values().map(BTreeSet::len).sum()
}

fn offer(group: &Group, files: &Seasons) -> Offered {
    Offered {
        group_id: group.id.clone(),
        name: group.name.clone(),
        kind: group.kind,
        seasons: group
            .seasons
            .iter()
            .map(|(&season, e)| SeasonCount {
                season,
                episodes: e.len(),
            })
            .collect(),
        exact: !files.is_empty() && group.seasons == *files,
        covered: covered(files, &group.seasons),
    }
}

/// The series' fetched groups, each with how it fits the files.
pub async fn offered(pool: &SqlitePool, tmdb_id: i64) -> sqlx::Result<Vec<Offered>> {
    let mut conn = pool.acquire().await?;
    let files = file_episodes(&mut conn, tmdb_id).await?;
    let groups = groups(&mut conn, tmdb_id).await?;
    Ok(groups.iter().map(|g| offer(g, &files)).collect())
}

/// The group closest to the files: the one holding most of their episodes (more than TMDB's own
/// order does), then the one with fewest episodes beyond them, then by id.
fn closest<'a>(files: &Seasons, tmdb: &Seasons, groups: &'a [Group]) -> Option<&'a Group> {
    let own = covered(files, tmdb);
    groups
        .iter()
        .filter(|g| covered(files, &g.seasons) > own)
        .min_by_key(|g| {
            let has = covered(files, &g.seasons);
            (usize::MAX - has, total(&g.seasons) - has, g.id.clone())
        })
}

/// The group to suggest for a series that follows TMDB's own order because nobody chose, when
/// its files use seasons that order doesn't have.
pub async fn suggestion(pool: &SqlitePool, tmdb_id: i64) -> sqlx::Result<Option<Offered>> {
    let chosen: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM series_orders WHERE series_tmdb_id = ?")
            .bind(tmdb_id)
            .fetch_optional(pool)
            .await?;
    if chosen.is_some() {
        return Ok(None);
    }
    let mut conn = pool.acquire().await?;
    let files = file_episodes(&mut conn, tmdb_id).await?;
    let tmdb = tmdb_seasons(&mut conn, tmdb_id).await?;
    if !needs_another(&files, &tmdb) {
        return Ok(None);
    }
    let groups = groups(&mut conn, tmdb_id).await?;
    Ok(closest(&files, &tmdb, &groups).map(|g| offer(g, &files)))
}

/// Switches each series nobody chose an order for whose files use seasons TMDB's own order
/// doesn't have to the one group that has exactly the files' seasons and episodes, when exactly
/// one does. Returns how many it switched.
pub async fn decide(pool: &SqlitePool) -> anyhow::Result<usize> {
    let series: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT s.match_tmdb_id FROM series s
         WHERE s.missing_since IS NULL AND s.match_tmdb_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM series_orders o WHERE o.series_tmdb_id = s.match_tmdb_id)
           AND EXISTS (SELECT 1 FROM tmdb_episode_groups g
                       WHERE g.series_tmdb_id = s.match_tmdb_id AND g.fetched_at IS NOT NULL)",
    )
    .fetch_all(pool)
    .await?;
    let mut switched = 0;
    for tmdb_id in series {
        // One series' trouble doesn't stop the rest.
        match decide_one(pool, tmdb_id).await {
            Ok(true) => switched += 1,
            Ok(false) => {}
            Err(err) => tracing::warn!(
                tmdb_id,
                error = format!("{err:#}"),
                "couldn't choose a series' episode order; will try again next run"
            ),
        }
    }
    Ok(switched)
}

/// Switches one series to the group its files fit exactly, when exactly one does; whether it did.
async fn decide_one(pool: &SqlitePool, tmdb_id: i64) -> anyhow::Result<bool> {
    let mut conn = pool.acquire().await?;
    let files = file_episodes(&mut conn, tmdb_id).await?;
    let tmdb = tmdb_seasons(&mut conn, tmdb_id).await?;
    if !needs_another(&files, &tmdb) {
        return Ok(false);
    }
    let groups = groups(&mut conn, tmdb_id).await?;
    drop(conn);
    let fitting: Vec<&Group> = groups.iter().filter(|g| g.seasons == files).collect();
    let [group] = fitting[..] else {
        return Ok(false);
    };
    let now = crate::db::unix_now();
    match choose(pool, tmdb_id, Some(&group.id), ChosenBy::Auto, now).await? {
        Chosen::Done => {
            tracing::info!(
                tmdb_id,
                group = group.name,
                "a series follows a TMDB episode group that fits its files"
            );
            Ok(true)
        }
        Chosen::Busy((season, episode)) => {
            tracing::info!(
                tmdb_id,
                season,
                episode,
                "a series waits to follow the episode group its files fit: a download is on its way"
            );
            Ok(false)
        }
        Chosen::NoSuchGroup | Chosen::EarlierStands => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seasons(counts: &[(i64, i64)]) -> Seasons {
        counts
            .iter()
            .map(|&(s, n)| (s, (1..=n).collect()))
            .collect()
    }

    fn group(id: &str, counts: &[(i64, i64)]) -> Group {
        Group {
            id: id.into(),
            name: id.into(),
            kind: Some(6),
            seasons: seasons(counts),
        }
    }

    #[test]
    fn the_closest_group_holds_most_of_the_files() {
        let files = seasons(&[(1, 24), (2, 22), (3, 12)]);
        let tmdb = seasons(&[(1, 59)]);
        assert!(needs_another(&files, &tmdb));
        assert!(!needs_another(&seasons(&[(1, 30)]), &tmdb));
        let groups = [
            group("a", &[(1, 59)]),
            group("b", &[(1, 24), (2, 23), (3, 12)]),
            group("c", &[(1, 24), (2, 23), (3, 12), (4, 10)]),
            group("d", &[(1, 12), (2, 12)]),
        ];
        // b and c hold all 58; b has fewer beyond them.
        assert_eq!(closest(&files, &tmdb, &groups).unwrap().id, "b");
        let offered = offer(&groups[1], &files);
        assert!(!offered.exact, "one episode of season 2 isn't here");
        assert_eq!(offered.covered, 58);
        // Nothing holds more than TMDB's own order: nothing to suggest.
        assert!(closest(&files, &tmdb, &groups[..1]).is_none());
    }
}
