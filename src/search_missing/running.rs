//! A run: one search at a time at the pace, leaving out indexers that reach their limit.

use super::*;

/// Whether an indexer's answer says it has reached its limit: HTTP 429, or a Torznab error
/// about a limit (codes 500 and 501 are the request and download limits).
pub(crate) fn says_limit(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("answered 429") || (error.contains("reported error") && error.contains("limit"))
}

pub(super) enum Ended {
    Done(String),
    Stopped(String),
}

#[derive(Default)]
pub(super) struct Counts {
    pub(super) done: i64,
    pub(super) searched: i64,
    pub(super) grabbed: i64,
    pub(super) notes: Vec<String>,
    /// Notes past those kept.
    pub(super) more_notes: usize,
}

impl Counts {
    pub(super) fn note(&mut self, note: String) {
        if self.notes.len() < NOTES_KEPT {
            self.notes.push(note);
        } else {
            self.more_notes += 1;
        }
    }

    pub(super) fn summary(&self, total: usize) -> String {
        let mut said = format!(
            "{} of {total} searched for, {} grabbed",
            self.searched, self.grabbed
        );
        if self.more_notes > 0 {
            said.push_str(&format!(" ({} more notes not kept)", self.more_notes));
        }
        said
    }
}

pub(super) async fn record(
    pool: &SqlitePool,
    id: i64,
    counts: &Counts,
    current: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE missing_searches SET done = ?, searched = ?, grabbed = ?, notes = ?, current = ?
         WHERE id = ?",
    )
    .bind(counts.done)
    .bind(counts.searched)
    .bind(counts.grabbed)
    .bind(serde_json::to_string(&counts.notes)?)
    .bind(current)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Waits until `until`, ending early when the run is asked to stop; true when it was.
pub(super) async fn wait_or_stop(pool: &SqlitePool, id: i64, until: Instant) -> sqlx::Result<bool> {
    loop {
        if stopping(pool, id).await? {
            return Ok(true);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        tokio::time::sleep(left.min(Duration::from_secs(1))).await;
    }
}

pub(super) async fn run(
    pool: &SqlitePool,
    sources: &Sources,
    id: i64,
    area: Area,
    goal: Goal,
) -> anyhow::Result<Ended> {
    let mut items: VecDeque<Item> = to_do(pool, area, goal).await?.into();
    let mut total = items.len();
    sqlx::query("UPDATE missing_searches SET total = ? WHERE id = ?")
        .bind(i64::try_from(total)?)
        .bind(id)
        .execute(pool)
        .await?;
    let client = torznab::Client::new()?;
    let music = artist_search::Sources {
        slskd: sources.slskd.clone(),
        pipeline: sources.pipeline.clone(),
    };
    let grabbing = match area {
        Area::Albums => music.pipeline.is_some() || music.slskd.is_some(),
        _ => sources.pipeline.is_some(),
    };
    let mut counts = Counts::default();
    if !grabbing {
        counts.note(
            "downloads are off (MELETE_DOWNLOADS): results are kept, nothing is grabbed".into(),
        );
    }
    let mut next_search = Instant::now();
    // Indexers found at their limit: left out of the rest of the run, by id and by name.
    let mut skipped: Vec<i64> = Vec::new();
    let mut limited: Vec<String> = Vec::new();
    while let Some(item) = items.pop_front() {
        if wait_or_stop(pool, id, next_search).await? {
            return Ok(Ended::Stopped(format!(
                "asked to stop; {}",
                counts.summary(total)
            )));
        }
        let name = item.name();
        if !still_to_do(pool, goal, &item).await? {
            counts.done += 1;
            record(pool, id, &counts, None).await?;
            continue;
        }
        record(pool, id, &counts, Some(&name)).await?;
        let searched = search_item(pool, &client, sources, &music, &item, &skipped).await?;
        counts.done += 1;
        let search_id = match searched {
            Ok(search_id) => search_id,
            // Nothing else in the area can be searched for either.
            Err(Refused::NoIndexers) if !limited.is_empty() => {
                record(pool, id, &counts, None).await?;
                return Ok(Ended::Stopped(format!(
                    "every indexer has reached its limit ({}); start again later, or turn one \
                     off under Indexers; {}",
                    limited.join(", "),
                    counts.summary(total)
                )));
            }
            Err(Refused::NoIndexers) => {
                record(pool, id, &counts, None).await?;
                return Ok(Ended::Stopped(format!(
                    "{}; {}",
                    Refused::NoIndexers,
                    counts.summary(total)
                )));
            }
            Err(refused) => {
                counts.note(format!("{name}: {refused}"));
                record(pool, id, &counts, None).await?;
                continue;
            }
        };
        // It went out to the indexers: the next waits its turn. A refused one asked no one.
        next_search = Instant::now() + sources.pace;
        counts.searched += 1;
        let (grabbed, said) = grab_item(pool, sources, &music, grabbing, &item, search_id).await?;
        let got = grabbed.iter().any(|g| g.download_id.is_some());
        // A torrent already on its way for it is as good as grabbed: no episodes instead.
        if !got && !said.already && grabbing && goal == Goal::Missing {
            total += episodes_next(pool, id, &item, &mut items, &mut counts).await?;
        }
        if got {
            counts.grabbed += 1;
            if let Some(took) = said.took {
                counts.note(format!("{name}: {took}"));
            }
        } else if let Some(why) = (grabbed.iter().rev().find_map(|g| g.refused.clone()))
            .or_else(|| said.passed_over.into_iter().next())
        {
            counts.note(format!("{name}: not grabbed: {why}"));
        }
        record(pool, id, &counts, None).await?;
        leave_out_limited(pool, id, search_id, &mut counts, &mut skipped, &mut limited).await?;
    }
    Ok(Ended::Done(counts.summary(total)))
}

/// After a missing season's search grabbed nothing: its missing episodes, each searched for
/// next on its own, with a note saying so. Returns how many were added.
async fn episodes_next(
    pool: &SqlitePool,
    id: i64,
    item: &Item,
    items: &mut VecDeque<Item>,
    counts: &mut Counts,
) -> anyhow::Result<usize> {
    let instead = episodes_instead(pool, item).await?;
    if instead.is_empty() {
        return Ok(0);
    }
    let numbers: Vec<i64> = instead
        .iter()
        .filter_map(|i| match i {
            Item::Episode { episode, .. } => Some(*episode),
            _ => None,
        })
        .collect();
    counts.note(format!(
        "{}: no pack was grabbed; searching for episodes {} on their own",
        item.name(),
        crate::rss::numbers(&numbers)
    ));
    let added = instead.len();
    for episode in instead.into_iter().rev() {
        items.push_front(episode);
    }
    sqlx::query("UPDATE missing_searches SET total = total + ? WHERE id = ?")
        .bind(i64::try_from(added)?)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(added)
}

/// Searches for one item as its own search would, leaving out the indexers in `skipped`.
async fn search_item(
    pool: &SqlitePool,
    client: &torznab::Client,
    sources: &Sources,
    music: &artist_search::Sources,
    item: &Item,
    skipped: &[i64],
) -> anyhow::Result<Result<i64, Refused>> {
    Ok(match (item, item.target()) {
        (Item::Album { id, .. }, _) => {
            album_search::search_skipping(pool, client, music.slskd.as_deref(), *id, skipped)
                .await?
        }
        (_, Some(target)) => {
            search::search_skipping(pool, client, sources.tmdb.as_deref(), target, skipped).await?
        }
        (_, None) => Err(Refused::NotWanted),
    })
}

/// Grabs the best accepted result of an item's search, when downloads are on for it; with why
/// results were passed over for what's on disk or on its way, and what was taken from a pack.
async fn grab_item(
    pool: &SqlitePool,
    sources: &Sources,
    music: &artist_search::Sources,
    grabbing: bool,
    item: &Item,
    search_id: i64,
) -> anyhow::Result<(Vec<Grabbed>, crate::rss::Said)> {
    let mut grabbed: Vec<Grabbed> = Vec::new();
    let mut said = crate::rss::Said::default();
    match (item, item.target(), &sources.pipeline) {
        (Item::Album { .. }, ..) if grabbing => {
            artist_search::grab_best(pool, music, search_id, &mut grabbed).await?;
        }
        (_, Some(target), Some(pipeline)) => {
            said = crate::rss::grab_for(pool, pipeline, target, search_id, &mut grabbed).await?;
        }
        _ => {}
    }
    Ok((grabbed, said))
}

/// Leaves the indexers that said in this search they reached their limit out of the rest of
/// the run, with a note for each.
async fn leave_out_limited(
    pool: &SqlitePool,
    id: i64,
    search_id: i64,
    counts: &mut Counts,
    skipped: &mut Vec<i64>,
    limited: &mut Vec<String>,
) -> anyhow::Result<()> {
    // An indexer at its limit isn't asked again by this run; the rest carry on.
    let indexers: String = sqlx::query_scalar("SELECT indexers FROM searches WHERE id = ?")
        .bind(search_id)
        .fetch_one(pool)
        .await?;
    let outcomes: Vec<IndexerOutcome> = serde_json::from_str(&indexers)?;
    for outcome in outcomes {
        if outcome.error.as_deref().is_some_and(says_limit)
            && !skipped.contains(&outcome.indexer_id)
        {
            skipped.push(outcome.indexer_id);
            counts.note(format!(
                "{} says it has reached its limit; not asked again by this run",
                outcome.name
            ));
            limited.push(outcome.name);
            record(pool, id, counts, None).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{admin, finished, sources};
    use super::*;
    use crate::config::Secret;
    use crate::deluge;
    use crate::downloads::Fetcher;
    use crate::rss::tests::{indexer, releases, setup};
    use sha2::{Digest, Sha256};

    /// "Some Anime" in its group's seasons, every episode aired, with season 2 wanted and its
    /// first `on_disk` episodes in the library; an indexer answering `feed`, and Deluge.
    async fn some_anime(pool: &SqlitePool, on_disk: i64, feed: &[&str]) -> Arc<Pipeline> {
        crate::episode_order::testing::some_anime_in_seasons(pool).await;
        sqlx::raw_sql(
            "UPDATE tmdb_episodes SET air_date = '2020-01-01' WHERE series_tmdb_id = 100;
             INSERT INTO libraries (id, name, kind, path) VALUES (1, 'anime', 'shows', '/a');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
                     match_tmdb_id)
                 VALUES (3, 1, 'Some Anime', 'Some Anime', 1, 100);
             INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type,
                     quality_profile, source, added_at, series_id, original_language)
                 VALUES (1, 200, 100, 'Some Anime', 1, 'anime', '4K Preferred', 'sonarr', 0, 3,
                     'en');
             INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (1, 2, 1);",
        )
        .execute(pool)
        .await
        .unwrap();
        for episode in 1..=on_disk {
            sqlx::query(
                "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id,
                     season, episode, first_seen_generation, seen_generation)
                 VALUES (1, ?, 1, 0, 3, 2, ?, 1, 1)",
            )
            .bind(format!(
                "Some Anime/Some.Anime.S02E{episode:02}.2160p.BluRay.REMUX.HEVC-GRP.mkv"
            ))
            .bind(episode)
            .execute(pool)
            .await
            .unwrap();
        }
        let indexer = indexer(pool, "[5000]").await;
        let items: Vec<(&str, &str)> = feed.iter().map(|t| (*t, "")).collect();
        indexer.lock().unwrap().0 = releases(&items);
        let (url, _fake) = deluge::testing::start().await;
        Arc::new(Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: Fetcher::new().unwrap(),
            seedbox: None,
        })
    }

    type Row = (String, Option<i64>, Option<i64>, Option<String>);

    /// Each download's title, season, episode and the episodes it was taken for.
    async fn downloads(pool: &SqlitePool) -> Vec<Row> {
        sqlx::query_as("SELECT title, season, episode, episodes FROM downloads ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn missing_episodes_no_pack_holds_are_searched_for_one_by_one() {
        let pool = setup().await;
        let who = admin(&pool).await;
        // Season 2 has 23 episodes, all on disk but the last two; no pack is out.
        let (e22, e23) = (
            "[SubsPlease] Some Anime - 46 (1080p) [ABCD1234].mkv",
            "[SubsPlease] Some Anime - 47 (1080p) [ABCD1234].mkv",
        );
        let pipeline = some_anime(&pool, 21, &[e22, e23]).await;
        let run = start(&pool, sources(Some(pipeline)), Area::Series, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!(
            (run.state.as_str(), run.total, run.searched, run.grabbed),
            ("done", 3, 3, 2),
            "{run:?}"
        );
        assert_eq!(
            run.notes,
            [
                "Some Anime, season 2: no pack was grabbed; searching for episodes 22, 23 on their own"
            ]
        );
        let searched: Vec<(i64, Option<i64>)> =
            sqlx::query_as("SELECT season, episode FROM searches ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(searched, [(2, None), (2, Some(22)), (2, Some(23))]);
        assert_eq!(
            downloads(&pool).await,
            [
                (e22.to_owned(), Some(2), Some(22), None),
                (e23.to_owned(), Some(2), Some(23), None)
            ]
        );
    }

    #[tokio::test]
    async fn a_torrent_already_on_its_way_for_the_season_is_as_good_as_grabbed() {
        let pool = setup().await;
        let who = admin(&pool).await;
        let pipeline = some_anime(&pool, 0, &[]).await;
        sqlx::query(
            "INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Season 1's pack is season 2's too, under another name: one torrent.
        let item = |title: &str, hash: &str, n: usize| {
            format!(
                r#"<item><title>{title}</title><guid>https://tracker.example/{n}</guid>
                <link>magnet:?xt=urn:btih:{hash}</link><size>5000000000</size>
                <torznab:attr name="seeders" value="50"/></item>"#
            )
        };
        let (both, single) = ("a".repeat(40), "b".repeat(40));
        let feed = indexer(&pool, "[5000]").await;
        feed.lock().unwrap().0 = format!(
            r#"<rss xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>{}{}{}</channel></rss>"#,
            item("Some.Anime.S01.1080p.BluRay.x264-GRP", &both, 1),
            item("Some.Anime.S02.1080p.BluRay.x264-GRP", &both, 2),
            item(
                "[SubsPlease] Some Anime - 46 (1080p) [ABCD1234].mkv",
                &single,
                3
            ),
        );
        let run = start(&pool, sources(Some(pipeline)), Area::Series, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!(
            (run.state.as_str(), run.total, run.searched, run.grabbed),
            ("done", 2, 2, 1),
            "{run:?}"
        );
        assert_eq!(
            run.notes,
            ["Some Anime, season 2: not grabbed: it's already download 1"]
        );
        let titles: Vec<String> = downloads(&pool).await.into_iter().map(|d| d.0).collect();
        assert_eq!(titles, ["Some.Anime.S01.1080p.BluRay.x264-GRP"]);
    }

    #[tokio::test]
    async fn a_season_whose_pack_failed_before_has_its_episodes_searched_for() {
        let pool = setup().await;
        let who = admin(&pool).await;
        // Episodes 1 to 21 are on disk; the season's pack failed in an earlier run (a dead
        // torrent, say) and is all an indexer still offers for the season.
        let pack = "Some.Anime.S02.1080p.BluRay.x264-GRP";
        let e22 = "[SubsPlease] Some Anime - 46 (1080p) [ABCD1234].mkv";
        let pipeline = some_anime(&pool, 21, &[pack, e22]).await;
        let hash: String = Sha256::digest(pack.as_bytes())[..20]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        sqlx::query(
            "INSERT INTO downloads (infohash, title, wanted_series_id, season, decision, state,
                 added_at, updated_at, episodes)
             VALUES (?, ?, 1, 2, '{}', 'failed', 0, 0, '[22, 23]')",
        )
        .bind(hash)
        .bind(pack)
        .execute(&pool)
        .await
        .unwrap();
        let run = start(&pool, sources(Some(pipeline)), Area::Series, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!(
            (run.state.as_str(), run.total, run.searched, run.grabbed),
            ("done", 3, 3, 1),
            "{run:?}"
        );
        let titles: Vec<String> = downloads(&pool).await.into_iter().map(|d| d.0).collect();
        assert_eq!(titles, [pack, e22]);
    }

    #[tokio::test]
    async fn a_pack_of_several_seasons_on_its_way_leaves_no_episode_to_search_for() {
        let pool = setup().await;
        some_anime(&pool, 0, &[]).await;
        assert_eq!(
            crate::rss::missing_episodes(&pool, 1, 2).await.unwrap(),
            (1..=23).collect::<Vec<i64>>()
        );
        // Grabbed for season 1, it holds season 2 as well.
        sqlx::query(
            "INSERT INTO downloads (infohash, title, wanted_series_id, season, decision, state,
                 added_at, updated_at)
             VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Some.Anime.S01-S02.1080p',
                 1, 1, '{}', 'downloading', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            crate::rss::missing_episodes(&pool, 1, 2).await.unwrap(),
            Vec::<i64>::new()
        );
    }

    #[tokio::test]
    async fn a_season_missing_whole_takes_the_pack() {
        let pool = setup().await;
        let who = admin(&pool).await;
        let pack = "Some.Anime.S02.1080p.BluRay.x264-GRP";
        let pipeline = some_anime(
            &pool,
            0,
            &[pack, "[SubsPlease] Some Anime - 46 (1080p) [ABCD1234].mkv"],
        )
        .await;
        let run = start(&pool, sources(Some(pipeline)), Area::Series, who)
            .await
            .unwrap()
            .unwrap();
        let run = finished(&pool, run.id).await;
        assert_eq!(
            (run.state.as_str(), run.total, run.searched, run.grabbed),
            ("done", 1, 1, 1),
            "{run:?}"
        );
        assert!(run.notes.is_empty(), "{run:?}");
        assert_eq!(
            downloads(&pool).await,
            [(pack.to_owned(), Some(2), None, None)]
        );
    }
}
