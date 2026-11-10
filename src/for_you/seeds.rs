//! What a list is made from: the recordings and artists a listener finished most lately, by
//! their own plays alone.

use super::*;
use crate::library::ARTIST_MBID;

/// Seed recordings a list is made from: enough to cover what someone plays, few enough that a
/// list is some 20 requests before its answers are kept.
pub(super) const RECORDINGS: usize = 10;
/// Seed artists.
pub(super) const ARTISTS: usize = 10;

/// A recording or artist a list is made from.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Seed {
    /// Its MusicBrainz id: all that's sent of it.
    pub mbid: String,
    /// What it's shown as: the song's title and artist, or the artist's name.
    pub because: Because,
    /// How strongly it figures in the listener's plays: its finished plays against the most
    /// of any seed of its kind, 0 to 1.
    pub weight: f64,
}

/// A listener's seeds.
#[derive(Debug, Default)]
pub(super) struct Seeds {
    /// The recordings, strongest first.
    pub recordings: Vec<Seed>,
    /// The artists, strongest first.
    pub artists: Vec<Seed>,
}

/// Whether `user` finished anything in the last 90 days.
pub(super) async fn listened(pool: &SqlitePool, user: i64, now: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM music_plays
             WHERE user_id = ? AND completed = 1 AND started_at >= ?)",
    )
    .bind(user)
    .bind(now * 1000 - SEED_WINDOW_MS)
    .fetch_one(pool)
    .await
}

/// SQL for `user`'s finished plays in the window, of present tracks `access` sees and they
/// haven't flagged, as `music_plays p` joined to `music_tracks t` and then `joins`; binds the
/// user, the window's start and the flagged tracks (a JSON array), in that order.
fn finished(access: &Access, joins: &str) -> String {
    format!(
        "FROM music_plays p JOIN music_tracks t ON t.id = p.track_id AND t.missing_since IS NULL
         {joins}
         WHERE p.user_id = ?1 AND p.completed = 1 AND p.started_at >= ?2 AND {}
           AND t.id NOT IN (SELECT value FROM json_each(?3))",
        access.track_sql("t")
    )
}

/// The best `most` of `rows` (an id, what it's shown as, finished plays, the last one's
/// start), each id once, most played first, then played last, then by id; weighed against the
/// most played.
fn strongest(rows: Vec<(String, Because, i64, i64)>, most: usize) -> Vec<Seed> {
    let mut rows: Vec<_> = rows
        .into_iter()
        .filter(|(mbid, ..)| crate::library::tags::is_mbid(mbid))
        .collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(b.3.cmp(&a.3)).then(a.0.cmp(&b.0)));
    let mut seen = HashSet::new();
    rows.retain(|(mbid, ..)| seen.insert(mbid.clone()));
    rows.truncate(most);
    let top = rows.first().map_or(1, |r| r.2.max(1)) as f64;
    rows.into_iter()
        .map(|(mbid, because, plays, _)| Seed {
            mbid,
            because,
            weight: plays as f64 / top,
        })
        .collect()
}

/// `user`'s seeds: the [`RECORDINGS`] recordings and [`ARTISTS`] artists (each track's first
/// credited) they finished most in the last 90 days, of tracks `access` sees, with MusicBrainz
/// ids, none of the tracks in `flagged`.
pub(super) async fn read(
    pool: &SqlitePool,
    user: i64,
    access: &Access,
    flagged: &HashSet<i64>,
    now: i64,
) -> sqlx::Result<Seeds> {
    let since = now * 1000 - SEED_WINDOW_MS;
    let flagged = serde_json::to_string(flagged).unwrap_or_else(|_| "[]".into());
    let recordings: Vec<(String, String, String, i64, i64)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT coalesce(t.mb_recording_id, t.match_recording_id) AS mbid, min(t.title),
                 min(coalesce(t.artist, '')), count(*), max(p.started_at) {}
               AND coalesce(t.mb_recording_id, t.match_recording_id) IS NOT NULL
             GROUP BY mbid",
            finished(access, "")
        )))
        .bind(user)
        .bind(since)
        .bind(&flagged)
        .fetch_all(pool)
        .await?;
    let artists: Vec<(Option<String>, String, i64, i64)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {ARTIST_MBID}, ar.name, count(*), max(p.started_at) {} GROUP BY ar.id",
            finished(
                access,
                "JOIN music_track_artists ta ON ta.track_id = t.id AND ta.position = 0
                 JOIN music_artists ar ON ar.id = ta.artist_id"
            )
        )))
        .bind(user)
        .bind(since)
        .bind(&flagged)
        .fetch_all(pool)
        .await?;
    let song = |title: String, artist: String| Because {
        kind: "song".into(),
        title: Some(title),
        artist,
    };
    Ok(Seeds {
        recordings: strongest(
            recordings
                .into_iter()
                .map(|(mbid, title, artist, plays, last)| (mbid, song(title, artist), plays, last))
                .collect(),
            RECORDINGS,
        ),
        artists: strongest(
            artists
                .into_iter()
                .filter_map(|(mbid, name, plays, last)| {
                    let because = Because {
                        kind: "artist".into(),
                        title: None,
                        artist: name,
                    };
                    Some((mbid?, because, plays, last))
                })
                .collect(),
            ARTISTS,
        ),
    })
}
