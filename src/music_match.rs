//! Matching music albums to MusicBrainz. An album whose tags name a release is
//! that release; otherwise MusicBrainz is searched by title and artist, and a result only counts
//! when its title and artist are the album's (case, punctuation and a leading "The" aside) and
//! it scores 90 or more. Results that all belong to one release group settle it; several groups
//! (narrowed first to editions with the album's track count) leave it ambiguous, with them as
//! candidates. A matched release's tracks give the album's tracks their MusicBrainz ids by disc
//! and position, when the track counts agree, in columns of their own (ids in tags stay as the
//! tags say). Scans clear those when a file changes; each run links such tracks again from the
//! cached release, without asking MusicBrainz.

use serde_json::json;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::musicbrainz::{self, Found, Release, credit_line};

mod by_hand;
mod deciding;
mod storing;

pub use by_hand::*;
use deciding::*;
use storing::*;

/// Errors in a row after which a run stops (MusicBrainz is down or refusing).
const MAX_ERRORS_IN_A_ROW: usize = 3;

/// What one matching run did, counted in albums.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Albums matched to a release.
    pub matched: usize,
    /// Albums MusicBrainz has nothing fitting for.
    pub unmatched: usize,
    /// Albums that fit several release groups, left for a choice by hand.
    pub ambiguous: usize,
    /// Albums whose lookup failed; a run stops after a few in a row.
    pub errors: usize,
}

#[derive(Debug, sqlx::FromRow)]
struct Album {
    id: i64,
    title: String,
    artist_id: Option<i64>,
    artist_name: String,
    compilation: bool,
    mb_release_id: Option<String>,
    tracks: i64,
}

enum Outcome {
    Matched {
        release: Box<Release>,
        method: &'static str,
    },
    Ambiguous(serde_json::Value),
    Unmatched,
}

/// Tries every present album not tried before.
pub async fn match_pending(pool: &SqlitePool, mb: &musicbrainz::Client) -> anyhow::Result<Report> {
    relink(pool).await?;
    let albums: Vec<Album> = sqlx::query_as(
        "SELECT a.id, a.title, a.artist_id, a.artist_name, a.compilation, a.mb_release_id,
             (SELECT count(*) FROM music_tracks t
                 WHERE t.album_id = a.id AND t.missing_since IS NULL) AS tracks
         FROM music_albums a
         WHERE a.missing_since IS NULL AND a.match_status IS NULL
         ORDER BY a.id",
    )
    .fetch_all(pool)
    .await?;
    let mut report = Report::default();
    let mut in_a_row = 0;
    for album in albums {
        let outcome = match decide(mb, &album).await {
            Ok(outcome) => outcome,
            Err(err) => {
                report.errors += 1;
                in_a_row += 1;
                tracing::warn!(album = album.title, error = %err, "couldn't match an album");
                if in_a_row >= MAX_ERRORS_IN_A_ROW {
                    tracing::warn!("MusicBrainz keeps failing; matching stops until the next run");
                    break;
                }
                continue;
            }
        };
        in_a_row = 0;
        let now = unix_now();
        let mut tx = pool.begin().await?;
        store_outcome(&mut tx, &album, outcome, now, &mut report).await?;
        tx.commit().await?;
    }
    // Releases cached before works, or tracks' artist credits, were kept are fetched again for
    // them, after new albums, and not when MusicBrainz is failing.
    if in_a_row < MAX_ERRORS_IN_A_ROW {
        let refetched = refetch_stale(pool, mb).await?;
        if refetched > 0 {
            tracing::info!(
                refetched,
                "fetched matched releases again for their works and track credits"
            );
        }
    }
    crate::jobs::WORKS.notify_one();
    Ok(report)
}

/// Records what matching decided for one album, and counts it.
async fn store_outcome(
    tx: &mut sqlx::SqliteConnection,
    album: &Album,
    outcome: Outcome,
    now: i64,
    report: &mut Report,
) -> anyhow::Result<()> {
    match outcome {
        Outcome::Matched { release, method } => {
            store_match(&mut *tx, album, &release, method, now).await?;
            report.matched += 1;
        }
        Outcome::Ambiguous(candidates) => {
            sqlx::query(
                "UPDATE music_albums SET match_status = 'ambiguous', match_method = NULL,
                         match_release_id = NULL, match_release_group_id = NULL,
                         match_candidates = ?, matched_at = ?
                     WHERE id = ?",
            )
            .bind(candidates.to_string())
            .bind(now)
            .bind(album.id)
            .execute(&mut *tx)
            .await?;
            report.ambiguous += 1;
        }
        Outcome::Unmatched => {
            sqlx::query(
                "UPDATE music_albums SET match_status = 'unmatched', match_method = NULL,
                         match_release_id = NULL, match_release_group_id = NULL,
                         match_candidates = NULL, matched_at = ?
                     WHERE id = ?",
            )
            .bind(now)
            .bind(album.id)
            .execute(&mut *tx)
            .await?;
            report.unmatched += 1;
        }
    }
    Ok(())
}

/// Records the release an album matched: the release itself, the album's match, its tracks'
/// ids, and the artist's MusicBrainz id when the release credits that artist alone.
async fn store_match(
    tx: &mut sqlx::SqliteConnection,
    album: &Album,
    release: &Release,
    method: &'static str,
    now: i64,
) -> anyhow::Result<()> {
    store_release(&mut *tx, release, now).await?;
    sqlx::query(
        "UPDATE music_albums SET match_status = 'matched', match_method = ?,
                         match_release_id = ?, match_release_group_id = ?, match_candidates = NULL,
                         matched_at = ?
                     WHERE id = ?",
    )
    .bind(method)
    .bind(&release.id)
    .bind(&release.release_group.id)
    .bind(now)
    .bind(album.id)
    .execute(&mut *tx)
    .await?;
    link_tracks(&mut *tx, album.id, &release.id).await?;
    if let (Some(artist_id), [credit]) = (album.artist_id, &release.artist_credit[..])
        && fold(&credit.artist.name) == fold(&album.artist_name)
    {
        sqlx::query(
            "UPDATE music_artists SET mb_artist_id = coalesce(mb_artist_id, ?) WHERE id = ?",
        )
        .bind(&credit.artist.id)
        .bind(artist_id)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

/// Fetches again the matched releases cached without their works or their tracks' artist
/// credits, one at a time at the client's pace.
async fn refetch_stale(pool: &SqlitePool, mb: &musicbrainz::Client) -> anyhow::Result<usize> {
    let releases: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT r.id FROM mb_releases r JOIN music_albums a ON a.match_release_id = r.id
         WHERE (r.with_works = 0 OR r.with_track_credits = 0)
           AND a.missing_since IS NULL AND a.match_status = 'matched'
         ORDER BY r.id",
    )
    .fetch_all(pool)
    .await?;
    let (mut done, mut in_a_row) = (0, 0);
    for id in releases {
        match mb.release(&id).await {
            Ok(release) => {
                in_a_row = 0;
                let mut tx = pool.begin().await?;
                store_release(&mut tx, &release, unix_now()).await?;
                tx.commit().await?;
                done += 1;
            }
            // Gone from MusicBrainz (merged, say): it keeps what it has, and isn't asked again.
            Err(musicbrainz::Error::NotFound) => {
                sqlx::query(
                    "UPDATE mb_releases SET with_works = 1, with_track_credits = 1 WHERE id = ?",
                )
                .bind(&id)
                .execute(pool)
                .await?;
            }
            Err(err) => {
                in_a_row += 1;
                tracing::warn!(release = id, error = %err, "couldn't fetch a release again");
                if in_a_row >= MAX_ERRORS_IN_A_ROW {
                    break;
                }
            }
        }
    }
    Ok(done)
}

/// Lower case, letters and digits only, without a leading "the": "The Beatles" and "beatles"
/// alike, "AC/DC" and "ACDC" alike.
pub(crate) fn fold(text: &str) -> String {
    let lower = text.to_lowercase();
    let lower = lower.strip_prefix("the ").unwrap_or(&lower);
    lower.chars().filter(|c| c.is_alphanumeric()).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::musicbrainz::testing;

    fn found(json: &str) -> Vec<Found> {
        #[derive(serde::Deserialize)]
        struct Search {
            releases: Vec<Found>,
        }
        serde_json::from_str::<Search>(json).unwrap().releases
    }

    #[test]
    fn one_release_group_settles_it() {
        let results = found(testing::SEARCH_OK);
        let Choice::One(id) = choose(&results, "OK Computer", Some("Radiohead"), 12) else {
            panic!("not settled");
        };
        let chosen = results.iter().find(|f| f.id == id).unwrap();
        assert_eq!(
            chosen.release_group.id,
            "b1392450-e666-3926-a536-22c65f834433"
        );
        assert_eq!(chosen.status.as_deref(), Some("Official"));
        // Another artist, or another title, isn't it.
        assert_eq!(
            choose(&results, "OK Computer", Some("Muse"), 12),
            Choice::None
        );
        assert_eq!(
            choose(&results, "Kid A", Some("Radiohead"), 12),
            Choice::None
        );
        // Folding: case, punctuation and "The".
        assert!(matches!(
            choose(&results, "ok computer!", Some("The Radiohead"), 12),
            Choice::One(_)
        ));
    }

    #[test]
    fn several_groups_are_narrowed_by_track_count_then_left_ambiguous() {
        let results = found(testing::SEARCH_AMBIGUOUS);
        let groups = |c: &Choice| match c {
            Choice::One(id) => vec![
                results
                    .iter()
                    .find(|f| &f.id == id)
                    .unwrap()
                    .release_group
                    .id[..8]
                    .to_owned(),
            ],
            Choice::Several(v) => v
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["release_group_id"].as_str().unwrap()[..8].to_owned())
                .collect(),
            Choice::None => vec![],
        };
        assert_eq!(
            groups(&choose(&results, "Greatest Hits", Some("Queen"), 17)),
            ["69ce61c8"]
        );
        assert_eq!(
            groups(&choose(&results, "Greatest Hits", Some("Queen"), 22)),
            ["015fa681"]
        );
        let neither = choose(&results, "Greatest Hits", Some("Queen"), 30);
        assert!(matches!(neither, Choice::Several(_)));
        assert_eq!(groups(&neither), ["69ce61c8", "015fa681"]);
        let Choice::Several(candidates) = neither else {
            unreachable!()
        };
        assert_eq!(candidates[0]["artist"], "Queen");
        assert_eq!(candidates[0]["tracks"], 17);
    }

    /// An album's id, status, method and release group.
    type MatchRow = (i64, Option<String>, Option<String>, Option<String>);

    async fn seeded() -> SqlitePool {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_artists (id, key, name) VALUES (1, 'name:radiohead', 'Radiohead'),
                 (2, 'name:queen', 'Queen');
             INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
                 mb_release_id, seen_generation) VALUES
                 (1, 1, 'mb:fba5f8fe-c6c8-4511-8562-c9febf482674', 'OK Computer', 1, 'Radiohead',
                     'r', 'fba5f8fe-c6c8-4511-8562-c9febf482674', 1),
                 (2, 1, 'dir:q', 'Greatest Hits', 2, 'Queen', 'q', NULL, 1),
                 (3, 1, 'dir:n', 'Nothing Like It', 2, 'Queen', 'n', NULL, 1),
                 (4, 1, 'dir:o', 'OK Computer', 1, 'Radiohead', 'o', NULL, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Album 1: twelve tracks numbered 1 to 12, the first with its own recording id.
        for n in 1..=12 {
            sqlx::query(
                "INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
                     first_seen_generation, seen_generation, album_id, tagged, title, track_number,
                     mb_recording_id)
                 VALUES (1, ?, 1, 1, 1, 1, 1, 1, 't', ?, ?)",
            )
            .bind(format!("r/{n}.flac"))
            .bind(n)
            .bind((n == 1).then_some("11111111-1111-1111-1111-111111111111"))
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    /// An album matched to the symphony's release before works were asked for, its four tracks
    /// linked, and one matched to a release MusicBrainz no longer has.
    pub(crate) async fn matched_before_works(pool: &SqlitePool) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO mb_releases (id, release_group_id, title, artist_credit, fetched_at)
                 VALUES ('{c}', 'g', 'Symphony No. 5', '[]', 0),
                        ('00000000-0000-0000-0000-00000000dead', 'g2', 'Gone', '[]', 0);
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                 seen_generation, match_status, match_method, match_release_id) VALUES
                 (1, 1, 'k1', 'Symphony No. 5', 'Wiener Philharmoniker', 's', 1, 'matched',
                     'search', '{c}'),
                 (2, 1, 'k2', 'Gone', 'Nobody', 'g', 1, 'matched', 'search',
                     '00000000-0000-0000-0000-00000000dead');",
            c = testing::CLASSICAL_ID
        )))
        .execute(pool)
        .await
        .unwrap();
        let release: Release = serde_json::from_str(testing::CLASSICAL).unwrap();
        for (n, track) in release.media[0].tracks.iter().enumerate() {
            sqlx::query(
                "INSERT INTO mb_release_tracks (release_id, disc, position, title, track_id,
                     recording_id) VALUES (?, 1, ?, ?, ?, ?)",
            )
            .bind(testing::CLASSICAL_ID)
            .bind(track.position)
            .bind(&track.title)
            .bind(&track.id)
            .bind(&track.recording.id)
            .execute(pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
                     first_seen_generation, seen_generation, album_id, tagged, title, track_number,
                     match_recording_id, match_track_id)
                 VALUES (1, ?, 1, 1, 1, 1, 1, 1, ?, ?, ?, ?)",
            )
            .bind(format!("s/{n}.flac"))
            .bind(&track.title)
            .bind(track.position)
            .bind(&track.recording.id)
            .bind(&track.id)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn releases_matched_before_works_are_fetched_again_for_them() {
        let pool = crate::db::open_in_memory().await;
        matched_before_works(&pool).await;
        let (mb, log) = testing::start(0).await;
        assert_eq!(match_pending(&pool, &mb).await.unwrap(), Report::default());
        assert_eq!(
            log.lock().unwrap().clone(),
            [
                "release 00000000-0000-0000-0000-00000000dead".to_owned(),
                format!("release {}", testing::CLASSICAL_ID)
            ]
        );
        let works: Vec<(String, Option<String>, Option<i64>, String)> = sqlx::query_as(
            "SELECT work_title, movement, movement_number, composers FROM mb_release_tracks
             WHERE release_id = ? ORDER BY position",
        )
        .bind(testing::CLASSICAL_ID)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(works.len(), 4);
        assert_eq!(works[1].0, "Symphony no. 5 in C minor, op. 67");
        assert_eq!(
            (works[1].1.as_deref(), works[1].2),
            (Some("Andante con moto"), Some(2))
        );
        assert!(works[1].3.contains("Ludwig van Beethoven"));
        // The tracks stay linked; neither release is asked for again, the gone one included.
        let linked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM music_tracks WHERE match_track_id IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, 4);
        match_pending(&pool, &mb).await.unwrap();
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    /// OK Computer cached with its works but before tracks' artist credits were kept, and
    /// another release cached since, with both.
    async fn matched_before_track_credits(pool: &SqlitePool) {
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO mb_releases (id, release_group_id, title, artist_credit, fetched_at,
                 with_works, with_track_credits) VALUES
                 ('fba5f8fe-c6c8-4511-8562-c9febf482674', 'g', 'OK Computer', '[]', 0, 1, 0),
                 ('00000000-0000-0000-0000-0000000000ab', 'g2', 'Since', '[]', 0, 1, 1);
             INSERT INTO mb_release_tracks (release_id, disc, position, title, track_id,
                 recording_id) VALUES
                 ('fba5f8fe-c6c8-4511-8562-c9febf482674', 1, 1, 'Airbag', 't1', 'r1');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                 seen_generation, match_status, match_method, match_release_id) VALUES
                 (1, 1, 'k1', 'OK Computer', 'Radiohead', 'r', 1, 'matched', 'search',
                     'fba5f8fe-c6c8-4511-8562-c9febf482674'),
                 (2, 1, 'k2', 'Since', 'Someone', 's', 1, 'matched', 'search',
                     '00000000-0000-0000-0000-0000000000ab');",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn releases_cached_before_track_credits_are_fetched_again_once_for_them() {
        let pool = crate::db::open_in_memory().await;
        matched_before_track_credits(&pool).await;
        let (mb, log) = testing::start(0).await;
        match_pending(&pool, &mb).await.unwrap();
        assert_eq!(
            log.lock().unwrap().clone(),
            ["release fba5f8fe-c6c8-4511-8562-c9febf482674"],
            "only the one without credits"
        );
        let credit: Option<String> = sqlx::query_scalar(
            "SELECT artist_credit FROM mb_release_tracks
             WHERE release_id = 'fba5f8fe-c6c8-4511-8562-c9febf482674' AND disc = 1
               AND position = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            credit.is_some_and(|c| c.contains(testing::RADIOHEAD)),
            "the track's credit is kept"
        );
        match_pending(&pool, &mb).await.unwrap();
        assert_eq!(log.lock().unwrap().len(), 1, "once");
    }

    #[tokio::test]
    async fn only_releases_cached_without_any_track_credit_are_marked_to_fetch_again() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let all = sqlx::migrate!();
        // Up to the one that keeps the flag, found by its name: numbers change at merge.
        let before = sqlx::migrate::Migrator {
            migrations: all
                .migrations
                .iter()
                .take_while(|m| m.description != "release track credits")
                .cloned()
                .collect(),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        before.run(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO mb_releases (id, release_group_id, title, artist_credit, fetched_at)
                 VALUES ('old', 'g', 'Old', '[]', 0), ('new', 'g', 'New', '[]', 0);
             INSERT INTO mb_release_tracks (release_id, disc, position, title, track_id,
                 recording_id, artist_credit) VALUES
                 ('old', 1, 1, 'A', 't1', 'r1', NULL),
                 ('new', 1, 1, 'B', 't2', 'r2', '[]');",
        )
        .execute(&pool)
        .await
        .unwrap();
        all.run(&pool).await.unwrap();
        let flags: Vec<(String, i64)> =
            sqlx::query_as("SELECT id, with_track_credits FROM mb_releases ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(flags, [("new".into(), 1), ("old".into(), 0)]);
    }

    #[tokio::test]
    async fn matches_by_tags_and_search_and_links_tracks() {
        let pool = seeded().await;
        let (mb, log) = testing::start(0).await;
        let report = match_pending(&pool, &mb).await.unwrap();
        assert_eq!(
            report,
            Report {
                matched: 2,
                unmatched: 1,
                ambiguous: 1,
                errors: 0
            }
        );

        let albums: Vec<MatchRow> = sqlx::query_as(
            "SELECT id, match_status, match_method, match_release_group_id FROM music_albums ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let group = Some("b1392450-e666-3926-a536-22c65f834433".to_owned());
        assert_eq!(
            albums,
            [
                (
                    1,
                    Some("matched".into()),
                    Some("tags".into()),
                    group.clone()
                ),
                (2, Some("ambiguous".into()), None, None),
                (3, Some("unmatched".into()), None, None),
                (4, Some("matched".into()), Some("search".into()), group),
            ]
        );
        // Tracks got the release's ids in their own columns; the tags' ids stay as they were.
        let ids: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT mb_recording_id, match_recording_id FROM music_tracks
             ORDER BY track_number LIMIT 2",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let release_first_two: Vec<String> = sqlx::query_scalar(
            "SELECT recording_id FROM mb_release_tracks WHERE disc = 1 AND position <= 2
             ORDER BY position",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            ids,
            [
                (
                    Some("11111111-1111-1111-1111-111111111111".into()),
                    Some(release_first_two[0].clone())
                ),
                (None, Some(release_first_two[1].clone())),
            ]
        );
        let artist: Option<String> =
            sqlx::query_scalar("SELECT mb_artist_id FROM music_artists WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(artist.is_some());
        let tracks: i64 = sqlx::query_scalar("SELECT count(*) FROM mb_release_tracks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tracks, 12);

        // Matched albums aren't asked about again; tracks whose files changed (a scan cleared
        // their links) are linked again from the cache, without asking either.
        sqlx::query("UPDATE music_tracks SET match_recording_id = NULL WHERE track_number > 6")
            .execute(&pool)
            .await
            .unwrap();
        let before = log.lock().unwrap().len();
        assert_eq!(match_pending(&pool, &mb).await.unwrap(), Report::default());
        assert_eq!(log.lock().unwrap().len(), before);
        let unlinked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM music_tracks WHERE album_id = 1 AND match_recording_id IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(unlinked, 0);
    }

    #[tokio::test]
    async fn keeps_each_release_tracks_own_credit() {
        let pool = seeded().await;
        let (mb, _log) = testing::start(0).await;
        match_pending(&pool, &mb).await.unwrap();
        // For working out tracks' artists; none when the answer had none.
        let credits: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT artist_credit FROM mb_release_tracks WHERE disc = 1 AND position <= 2
             ORDER BY position",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            credits,
            [
                Some(format!(
                    r#"[{{"artist":"Radiohead","id":"{}","join":"","name":"Radiohead"}}]"#,
                    testing::RADIOHEAD
                )),
                None
            ]
        );
    }

    #[tokio::test]
    async fn stops_when_musicbrainz_keeps_failing() {
        let pool = seeded().await;
        // Two busy answers per request: every album fails.
        let (mb, log) = testing::start(usize::MAX).await;
        let report = match_pending(&pool, &mb).await.unwrap();
        assert_eq!(report.errors, MAX_ERRORS_IN_A_ROW);
        assert_eq!(log.lock().unwrap().len(), MAX_ERRORS_IN_A_ROW * 2);
        let untried: i64 =
            sqlx::query_scalar("SELECT count(*) FROM music_albums WHERE match_status IS NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(untried, 4, "failures leave albums to try again");
    }
}
