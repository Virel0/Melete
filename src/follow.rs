//! Following artists: a wanted artist's rule for later albums (`monitor_new`: all, new
//! or none, as Lidarr has it) acted on. Now and then each followed artist's studio albums are
//! looked up on MusicBrainz, through the client the matching shares so together they keep to
//! one request a second. The first look only notes what's there; a release group that appears
//! after it is wanted, with "all" whatever its date, with "new" when it's released from the day
//! of that first look on (or has no date yet: an album announced). RSS and searches then fetch
//! it as any wanted album. What leaves the server: the artists' MusicBrainz ids, to MusicBrainz.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use sqlx::{SqliteConnection, SqlitePool};

use crate::db::unix_now;
use crate::musicbrainz::{self, ReleaseGroup};

/// How often an artist is looked up.
pub const EVERY_SECONDS: i64 = 86_400;
/// How often the job wakes to look for artists that are due.
const WAKE: Duration = Duration::from_secs(6 * 3600);

/// What a round of looking did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Followed artists looked up.
    pub checked: u64,
    /// New albums put on the wanted list.
    pub wanted: u64,
    /// Artists whose lookup failed; each is tried again when next due.
    pub failed: u64,
}

/// A day since 1970 as YYYY-MM-DD (Howard Hinnant's civil-from-days).
fn date(unix: i64) -> String {
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Whether a release date ("2026", "2026-05" or "2026-05-01") may fall on or after `since`: a
/// year or month alone counts by its last day. No date is an album still to come.
pub fn released_since(release: Option<&str>, since: &str) -> bool {
    let Some(release) = release.filter(|r| !r.is_empty()) else {
        return true;
    };
    let latest = match release.len() {
        4 => format!("{release}-12-31"),
        7 => format!("{release}-31"),
        _ => release.to_owned(),
    };
    latest.as_str() >= since
}

#[derive(sqlx::FromRow)]
struct Due {
    id: i64,
    mb_artist_id: String,
    monitor_new: String,
    since: Option<String>,
    looked: Option<bool>,
}

/// Looks up every followed artist not looked up in the last day, oldest first, with `albums`
/// (an artist's studio albums on MusicBrainz).
pub async fn check_due<F, Fut>(pool: &SqlitePool, albums: F, now: i64) -> sqlx::Result<Report>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<ReleaseGroup>, musicbrainz::Error>>,
{
    let due = due(pool, now).await?;
    let mut report = Report::default();
    for artist in due {
        report.checked += 1;
        let first = artist.looked != Some(true);
        let since = artist.since.clone().unwrap_or_else(|| date(now));
        let groups = match albums(artist.mb_artist_id.clone()).await {
            Ok(groups) => groups,
            Err(err) => {
                report.failed += 1;
                tracing::warn!(artist = artist.mb_artist_id, error = %err, "couldn't look up a followed artist's albums");
                note_failure(pool, artist.id, &since, now, err.to_string()).await?;
                continue;
            }
        };
        let mut tx = pool.begin().await?;
        for group in &groups {
            if want_if_new(&mut tx, &artist, group, first, &since, now).await? {
                report.wanted += 1;
                tracing::info!(
                    artist = artist.mb_artist_id,
                    album = group.title.as_deref().unwrap_or("Untitled"),
                    "a followed artist's new album is wanted"
                );
            }
        }
        note_looked(&mut tx, artist.id, &since, now).await?;
        tx.commit().await?;
    }
    Ok(report)
}

/// The followed artists due a look, oldest first.
async fn due(pool: &SqlitePool, now: i64) -> sqlx::Result<Vec<Due>> {
    let due: Vec<Due> = sqlx::query_as(
        "SELECT a.id, a.mb_artist_id, a.monitor_new, c.since, c.looked FROM wanted_artists a
         LEFT JOIN followed_artist_checks c ON c.wanted_artist_id = a.id
         WHERE a.monitored = 1 AND a.monitor_new IN ('all', 'new')
           AND (c.checked_at IS NULL OR c.checked_at <= ?)
         ORDER BY coalesce(c.checked_at, 0), a.id",
    )
    .bind(now - EVERY_SECONDS)
    .fetch_all(pool)
    .await?;
    Ok(due)
}

/// Records a look that failed, keeping the artist's first look still to come.
async fn note_failure(
    pool: &SqlitePool,
    artist_id: i64,
    since: &str,
    now: i64,
    error: String,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO followed_artist_checks (wanted_artist_id, since, checked_at,
                         looked, error) VALUES (?1, ?2, ?3, 0, ?4)
                     ON CONFLICT (wanted_artist_id) DO UPDATE SET checked_at = ?3, error = ?4",
    )
    .bind(artist_id)
    .bind(since)
    .bind(now)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Notes a release group as seen and, when it's new after the first look and the artist's rule
/// takes it, wants it; true when it was added to the wanted list.
async fn want_if_new(
    tx: &mut SqliteConnection,
    artist: &Due,
    group: &ReleaseGroup,
    first: bool,
    since: &str,
    now: i64,
) -> sqlx::Result<bool> {
    let seen = sqlx::query(
        "INSERT INTO followed_release_groups (wanted_artist_id, mb_release_group_id,
                     seen_at) VALUES (?, ?, ?)
                 ON CONFLICT DO NOTHING",
    )
    .bind(artist.id)
    .bind(&group.id)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 0;
    // The first look only notes what's there; after it, what's new to it is wanted
    // as the artist's rule says.
    if seen || first {
        return Ok(false);
    }
    let wanted = match artist.monitor_new.as_str() {
        "all" => true,
        "new" => released_since(group.first_release_date.as_deref(), since),
        _ => false,
    };
    if !wanted {
        return Ok(false);
    }
    let secondary = serde_json::to_string(group.secondary_types.as_deref().unwrap_or_default())
        .unwrap_or_else(|_| "[]".into());
    // An album already on the list, wanted or not, is left as it is.
    let added = sqlx::query(
        "INSERT INTO wanted_albums (wanted_artist_id, mb_release_group_id, title,
                     album_type, secondary_types, release_date, monitored, source, added_at)
                 VALUES (?, ?, ?, ?, ?, ?, 1, 'manual', ?)
                 ON CONFLICT (mb_release_group_id) DO NOTHING",
    )
    .bind(artist.id)
    .bind(&group.id)
    .bind(group.title.as_deref().unwrap_or("Untitled"))
    .bind(&group.primary_type)
    .bind(secondary)
    .bind(&group.first_release_date)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    Ok(added > 0)
}

/// Records a look that worked: the artist's first look is done.
async fn note_looked(
    tx: &mut SqliteConnection,
    artist_id: i64,
    since: &str,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO followed_artist_checks (wanted_artist_id, since, checked_at, looked,
                 error) VALUES (?1, ?2, ?3, 1, NULL)
             ON CONFLICT (wanted_artist_id) DO UPDATE SET checked_at = ?3, looked = 1,
                 error = NULL",
    )
    .bind(artist_id)
    .bind(since)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Looks for followed artists' new albums every few hours, while MusicBrainz is on.
pub async fn run(pool: SqlitePool, mb: Arc<musicbrainz::Client>) {
    crate::jobs::repeat_task("following", WAKE, &crate::jobs::FOLLOW, || async {
        let albums = |id: String| {
            let mb = mb.clone();
            async move { mb.artist_albums(&id).await }
        };
        match check_due(&pool, albums, unix_now()).await {
            Ok(report) if report.checked == 0 => {}
            Ok(report) => tracing::info!(
                checked = report.checked,
                wanted = report.wanted,
                failed = report.failed,
                "looked for followed artists' new albums"
            ),
            Err(err) => {
                tracing::error!(error = %err, "looking for followed artists' new albums stopped")
            }
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    fn group(id: &str, title: &str, date: Option<&str>) -> ReleaseGroup {
        ReleaseGroup {
            id: id.into(),
            title: Some(title.into()),
            primary_type: Some("Album".into()),
            secondary_types: Some(Vec::new()),
            first_release_date: date.map(str::to_owned),
        }
    }

    #[test]
    fn dates_and_what_counts_as_new() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_793_000_000), "2026-10-26");
        assert!(released_since(Some("2026-10-27"), "2026-10-27"));
        assert!(!released_since(Some("2026-10-26"), "2026-10-27"));
        assert!(
            released_since(Some("2026"), "2026-10-27"),
            "a year counts by its end"
        );
        assert!(released_since(Some("2026-10"), "2026-10-27"));
        assert!(!released_since(Some("2026-09"), "2026-10-27"));
        assert!(released_since(None, "2026-10-27"), "announced, no date yet");
    }

    async fn artists(pool: &SqlitePool) {
        sqlx::raw_sql(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source,
                 added_at) VALUES
                 (1, 'mb-all', 'Alder', 1, 'all', 'manual', 0),
                 (2, 'mb-new', 'Birch', 1, 'new', 'manual', 0),
                 (3, 'mb-none', 'Cedar', 1, 'none', 'manual', 0),
                 (4, 'mb-off', 'Dogwood', 0, 'all', 'manual', 0),
                 (5, 'mb-gone', 'Elm', 1, 'all', 'manual', 0);
             INSERT INTO wanted_albums (wanted_artist_id, mb_release_group_id, title, monitored,
                 source, added_at) VALUES (1, 'rg-dropped', 'Dropped', 0, 'manual', 0);",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn wanted(pool: &SqlitePool) -> Vec<(i64, String, bool)> {
        sqlx::query_as("SELECT wanted_artist_id, title, monitored FROM wanted_albums ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn new_release_groups_are_wanted_as_each_artist_says() {
        let pool = crate::db::open_in_memory().await;
        artists(&pool).await;
        let catalogue: Mutex<HashMap<String, Vec<ReleaseGroup>>> = Mutex::new(HashMap::from([
            (
                "mb-all".to_owned(),
                vec![group("rg-a1", "A1", Some("2001"))],
            ),
            (
                "mb-new".to_owned(),
                vec![group("rg-b1", "B1", Some("2002-03-04"))],
            ),
        ]));
        let asked: Mutex<Vec<String>> = Mutex::default();
        let albums = |id: String| {
            asked.lock().unwrap().push(id.clone());
            let found = catalogue.lock().unwrap().get(&id).cloned();
            async move { found.ok_or(musicbrainz::Error::NotFound) }
        };
        let day = 86_400;
        let start = 1_793_000_000; // 2026-10-26

        // The first look notes what's there and wants nothing; "none" and unfollowed artists
        // aren't asked about.
        let first = check_due(&pool, &albums, start).await.unwrap();
        assert_eq!(
            first,
            Report {
                checked: 3,
                wanted: 0,
                failed: 1
            }
        );
        assert_eq!(*asked.lock().unwrap(), ["mb-all", "mb-new", "mb-gone"]);
        assert_eq!(wanted(&pool).await.len(), 1);
        // Not again within the day.
        assert_eq!(
            check_due(&pool, &albums, start + 3600)
                .await
                .unwrap()
                .checked,
            0
        );

        // Later: Alder gets an old album added to MusicBrainz and the one dropped from the
        // list; Birch an old one, a new one and an announced one.
        {
            let mut catalogue = catalogue.lock().unwrap();
            catalogue.get_mut("mb-all").unwrap().extend([
                group("rg-a0", "A0", Some("1999")),
                group("rg-dropped", "Dropped", Some("2010")),
            ]);
            catalogue.get_mut("mb-new").unwrap().extend([
                group("rg-b0", "B0", Some("2000")),
                group("rg-b2", "B2", Some("2026-11-20")),
                group("rg-b3", "B3", None),
            ]);
        }
        let later = check_due(&pool, &albums, start + 2 * day).await.unwrap();
        assert_eq!(later.wanted, 3);
        assert_eq!(
            wanted(&pool).await,
            [
                (1, "Dropped".into(), false),
                (1, "A0".into(), true),
                (2, "B2".into(), true),
                (2, "B3".into(), true),
            ],
            "all: whatever its date; new: released since the first look, or announced; a \
             dropped album stays dropped"
        );
        // Seen once, never wanted twice.
        let again = check_due(&pool, &albums, start + 4 * day).await.unwrap();
        assert_eq!(again.wanted, 0);

        // An artist whose first look failed gets its first real look later: noting, not
        // wanting.
        catalogue.lock().unwrap().insert(
            "mb-gone".into(),
            vec![group("rg-e1", "E1", Some("2026-12-01"))],
        );
        let found = check_due(&pool, &albums, start + 6 * day).await.unwrap();
        assert_eq!((found.failed, found.wanted), (0, 0));
    }

    #[tokio::test]
    async fn looks_up_studio_albums_through_the_shared_client() {
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source,
                 added_at) VALUES (1, ?, 'Radiohead', 1, 'all', 'manual', 0)",
        )
        .bind(musicbrainz::testing::RADIOHEAD)
        .execute(&pool)
        .await
        .unwrap();
        let (mb, log) = musicbrainz::testing::start(0).await;
        let mb = Arc::new(mb);
        let albums = |id: String| {
            let mb = mb.clone();
            async move { mb.artist_albums(&id).await }
        };
        let report = check_due(&pool, albums, 1_793_000_000).await.unwrap();
        assert_eq!(
            report,
            Report {
                checked: 1,
                wanted: 0,
                failed: 0
            }
        );
        let seen: Vec<String> = sqlx::query_scalar(
            "SELECT mb_release_group_id FROM followed_release_groups ORDER BY mb_release_group_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            seen.len(),
            2,
            "the two studio albums, not the live one or the single"
        );
        let asked = log.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert!(
            asked[0].contains(musicbrainz::testing::RADIOHEAD),
            "{asked:?}"
        );
    }
}
