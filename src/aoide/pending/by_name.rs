//! A song the library has though not as the request's recording: a track whose title and
//! artist are the request's (as YouTube's matching reads them: a leading article, a featured
//! artist, a bracketed part or " - Remastered" left out; accents, case and "&" not counted),
//! and, when both lengths are known, as long within a few seconds; only when one track is. A song asked for by name
//! has no recording, and one already in the library under another of MusicBrainz's listings
//! of it, or untagged, matches no recording; either way it's in the library, and its items
//! stop waiting.

use std::collections::HashMap;

use serde_json::Value;
use sqlx::SqlitePool;

use crate::song_youtube::choosing::{core_title, names_artist, plain};

/// Seconds a track's length may differ from the song's.
const LENGTH_SLACK: f64 = 10.0;

#[derive(sqlx::FromRow)]
struct Asked {
    id: i64,
    title: String,
    artist: Option<String>,
    length_seconds: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct Track {
    id: i64,
    title: String,
    artist: Option<String>,
    duration_seconds: Option<f64>,
}

/// Whether a track's artist is the song's: the same name, or either naming the other (a
/// track by "Simon & Garfunkel" for a song by "Simon and Garfunkel", "Duo feat. X" for "Duo").
fn same_artist(track: &str, song: &str) -> bool {
    let (track, song) = (plain(track), plain(song));
    !track.is_empty()
        && !song.is_empty()
        && (track == song || names_artist(&track, &song) || names_artist(&song, &track))
}

fn same_length(track: Option<f64>, song: Option<i64>) -> bool {
    match (track, song) {
        (Some(track), Some(song)) => (track - song as f64).abs() <= LENGTH_SLACK,
        _ => true,
    }
}

/// The library's tracks by their titles as matching reads them; none a failed download left
/// (it may not be what it was taken for).
struct Index {
    tracks: Vec<Track>,
    by_title: HashMap<String, Vec<usize>>,
}

impl Index {
    async fn load(pool: &SqlitePool) -> sqlx::Result<Self> {
        let tracks: Vec<Track> = sqlx::query_as(
            "SELECT t.id, t.title, t.artist, t.duration_seconds FROM music_tracks t
             JOIN libraries l ON l.id = t.library_id
             WHERE t.missing_since IS NULL
               AND rtrim(l.path, '/') || '/' || t.rel_path NOT IN (
                   SELECT p.value FROM downloads d JOIN json_each(d.placed) p
                   WHERE d.state = 'failed' AND json_valid(d.placed))
             ORDER BY t.id",
        )
        .fetch_all(pool)
        .await?;
        let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
        for (at, track) in tracks.iter().enumerate() {
            let title = core_title(&track.title);
            if !title.is_empty() {
                by_title.entry(title).or_default().push(at);
            }
        }
        Ok(Self { tracks, by_title })
    }

    /// Every track that is the song.
    fn all(&self, song: &Asked) -> Vec<i64> {
        let Some(artist) = song.artist.as_deref() else {
            return Vec::new();
        };
        self.by_title
            .get(&core_title(&song.title))
            .into_iter()
            .flatten()
            .map(|at| &self.tracks[*at])
            .filter(|t| {
                t.artist.as_deref().is_some_and(|a| same_artist(a, artist))
                    && same_length(t.duration_seconds, song.length_seconds)
            })
            .map(|t| t.id)
            .collect()
    }

    /// The one track that is the song; none when several are, as which isn't known.
    fn one(&self, song: &Asked) -> Option<i64> {
        match self.all(song)[..] {
            [track] => Some(track),
            _ => None,
        }
    }
}

/// Which of `requests` the library has one track for by title and artist: each as (request,
/// the track).
pub(super) async fn arrived(pool: &SqlitePool, requests: &[i64]) -> sqlx::Result<Vec<(i64, i64)>> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let asked: Vec<Asked> = sqlx::query_as(
        "SELECT r.id, coalesce(s.title, r.title) AS title, coalesce(s.artist, r.artist) AS artist,
             s.length_seconds
         FROM requests r LEFT JOIN wanted_songs s ON s.id = r.wanted_song_id
         WHERE r.kind = 'song' AND r.id IN (SELECT value FROM json_each(?))",
    )
    .bind(Value::from(requests.to_vec()).to_string())
    .fetch_all(pool)
    .await?;
    if asked.is_empty() {
        return Ok(Vec::new());
    }
    let index = Index::load(pool).await?;
    Ok(asked
        .iter()
        .filter_map(|song| Some((song.id, index.one(song)?)))
        .collect())
}

/// Which of the wanted songs `songs` the library has already, whatever brought it: a track of
/// its recording, or with its title and artist. Nothing is fetched again for those.
pub(crate) async fn in_library(
    pool: &SqlitePool,
    songs: &[i64],
) -> sqlx::Result<std::collections::HashSet<i64>> {
    if songs.is_empty() {
        return Ok(Default::default());
    }
    let ids = Value::from(songs.to_vec()).to_string();
    let mut have: std::collections::HashSet<i64> = sqlx::query_scalar(
        "SELECT s.id FROM wanted_songs s
         WHERE s.id IN (SELECT value FROM json_each(?)) AND s.mb_recording_id IS NOT NULL
           AND EXISTS (SELECT 1 FROM music_tracks t WHERE t.missing_since IS NULL
                       AND coalesce(t.mb_recording_id, t.match_recording_id) = s.mb_recording_id)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let asked: Vec<Asked> = sqlx::query_as(
        "SELECT id, title, artist, length_seconds FROM wanted_songs
         WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let index = Index::load(pool).await?;
    have.extend(
        asked
            .iter()
            .filter(|s| !index.all(s).is_empty())
            .map(|s| s.id),
    );
    Ok(have)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_is_the_song_by_its_title_artist_and_length() {
        assert!(same_artist("Simon & Garfunkel", "Simon and Garfunkel"));
        assert!(same_artist("The Beatles", "Beatles"));
        assert!(same_artist("Duo feat. Someone", "Duo"));
        assert!(!same_artist("Someone Else", "Duo"));
        assert!(!same_artist("", "Duo"));
        assert_eq!(core_title("Song (2011 Remaster)"), core_title("Song"));
        assert_eq!(core_title("Song - Remastered"), core_title("song"));
        assert!(same_length(Some(200.0), Some(205)));
        assert!(!same_length(Some(200.0), Some(260)));
        assert!(same_length(None, Some(260)));
    }

    #[tokio::test]
    async fn a_request_whose_song_is_in_the_library_under_another_recording_has_arrived() {
        let pool = crate::db::open_in_memory().await;
        let owner = crate::auth::testing::user(&pool, "owner", false).await.id;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k', 'Album', 'The Duo', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres,
                 duration_seconds, mb_recording_id)
                 VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Song (Remastered)', 'The Duo', '[]', 200.0,
                         'other-recording'),
                        (8, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Long Song', 'The Duo', '[]', 400.0, NULL);
             INSERT INTO wanted_songs (id, title, artist, length_seconds, library_id, source, added_at)
                 VALUES (2, 'Song', 'Duo', 203, 1, 'manual', 0),
                        (3, 'Long Song', 'Duo', 200, 1, 'manual', 0);
             INSERT INTO requests (id, user_id, kind, title, artist, state, created_at, decided_at,
                 wanted_song_id, mb_recording_id)
                 VALUES (10, {owner}, 'song', 'Song', 'Duo', 'approved', 0, 0, 2, 'asked-recording'),
                        (11, {owner}, 'song', 'Long Song', 'Duo', 'approved', 0, 0, 3, NULL),
                        (12, {owner}, 'song', 'Missing', 'Duo', 'approved', 0, 0, NULL, NULL);"
        )))
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            arrived(&pool, &[10, 11, 12]).await.unwrap(),
            [(10, 7)],
            "the long song's length isn't the request's; the missing one isn't there"
        );
    }
}
