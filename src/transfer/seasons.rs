//! A pack of several seasons brought home one season at a time: the season it was grabbed for
//! first, as any season pack is, then each other season it was taken for (`downloads.seasons`),
//! each into its own folder and checked against its own episodes' running times, the download
//! queued again for the next until none is left. Packs brought home before for one season only,
//! whose files are still on the seedbox, are opened again for the other monitored seasons they
//! hold missing episodes of.

use std::collections::BTreeMap;

use super::*;

/// The other seasons a download still has to bring home, by season, with their episodes.
type Seasons = BTreeMap<i64, Vec<i64>>;

fn read(seasons: Option<&str>) -> Seasons {
    let by_key: BTreeMap<String, Vec<i64>> = seasons
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    by_key
        .into_iter()
        .filter_map(|(season, episodes)| Some((season.parse().ok()?, episodes)))
        .collect()
}

fn write(seasons: &Seasons) -> String {
    let by_key: BTreeMap<String, &Vec<i64>> =
        seasons.iter().map(|(s, e)| (s.to_string(), e)).collect();
    serde_json::to_string(&by_key).unwrap_or_else(|_| "{}".into())
}

/// After download `id` brought a season home: when it has another season to bring home, it's
/// queued again for that one (the lowest), its episodes and what's left noted, and that season
/// is returned; None when it's done.
pub(super) async fn queue_next(
    pool: &SqlitePool,
    id: i64,
    message: &str,
) -> anyhow::Result<Option<i64>> {
    let seasons: Option<String> = sqlx::query_scalar("SELECT seasons FROM downloads WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;
    let mut left = read(seasons.as_deref());
    let Some((season, episodes)) = left.pop_first() else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE downloads SET state = 'completed', season = ?, episode = NULL, episodes = ?,
             seasons = ?, attempts = 0, next_attempt_at = NULL, message = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(season)
    .bind(serde_json::to_string(&episodes)?)
    .bind(write(&left))
    .bind(format!("{message}; bringing home season {season} next"))
    .bind(unix_now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(Some(season))
}

/// An imported pack of several seasons, as reopening reads it.
#[derive(sqlx::FromRow)]
struct Imported {
    id: i64,
    title: String,
    wanted_series_id: i64,
    season: i64,
    decision: String,
}

/// Opens again each imported pack of several seasons brought home for one season only (none
/// noted in `seasons`), while its files are still on the seedbox: the other monitored seasons
/// it holds missing episodes of are noted and it's queued for the first, as a pack grabbed now
/// would be; one with none is noted as looked at (`{}`), so it isn't looked at again. How many
/// were opened again.
pub async fn reopen_packs(pool: &SqlitePool) -> anyhow::Result<usize> {
    let rows: Vec<Imported> = sqlx::query_as(
        "SELECT id, title, wanted_series_id, season, decision FROM downloads
         WHERE source = 'torrent' AND state = 'imported' AND seasons IS NULL
           AND wanted_series_id IS NOT NULL AND season IS NOT NULL AND episode IS NULL
           AND (deluge_removed IS NULL OR deluge_removed = 'kept_data')
         ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let mut opened = 0;
    for row in rows {
        let release = crate::release::parse(&row.title);
        let crate::release::Episodes::Seasons(in_it) = &release.episodes else {
            continue;
        };
        if in_it.len() < 2 {
            continue;
        }
        let target = Target::Series {
            id: row.wanted_series_id,
            season: row.season,
            episode: None,
        };
        let Some(wanted) = crate::search::load_wanted(pool, target).await? else {
            continue;
        };
        let decision: serde_json::Value = serde_json::from_str(&row.decision).unwrap_or_default();
        let others = crate::rss::others_of(
            pool,
            row.wanted_series_id,
            row.season,
            in_it,
            &wanted,
            &release,
            &decision,
        )
        .await?;
        let seasons: Seasons = others.into_iter().collect();
        sqlx::query("UPDATE downloads SET seasons = ? WHERE id = ? AND seasons IS NULL")
            .bind(write(&seasons))
            .bind(row.id)
            .execute(pool)
            .await?;
        if let Some(season) = queue_next(pool, row.id, "brought home before for one season").await?
        {
            tracing::info!(
                download = row.id,
                season,
                "bringing home the other seasons of a pack brought home for one"
            );
            opened += 1;
        }
    }
    Ok(opened)
}
