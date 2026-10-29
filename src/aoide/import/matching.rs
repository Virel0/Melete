//! Finding a record's track in Melete's library by the content key the apps built from its tags.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct TrackRow {
    pub(super) id: i64,
    pub(super) title: String,
    pub(super) artist: Option<String>,
    pub(super) album_artist: Option<String>,
    pub(super) album: Option<String>,
    pub(super) album_title: String,
    pub(super) album_artist_name: String,
    pub(super) duration_seconds: Option<f64>,
}

/// Present tracks under every `artist|album|title` the apps could have built for them from
/// Jellyfin's view of the same tags: the first or the whole artist, else the album artist,
/// else "Unknown Artist"; the album tag or the album's title.
pub(super) struct Library {
    pub(super) by_prefix: HashMap<String, Vec<(i64, i64)>>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Found {
    Exact(i64),
    /// A second either side: Jellyfin and Melete can round a duration differently.
    Near(i64),
    Ambiguous,
    Missing,
}

impl Library {
    pub(super) async fn load(pool: &SqlitePool) -> sqlx::Result<Self> {
        let rows: Vec<TrackRow> = sqlx::query_as(
            "SELECT t.id, t.title, t.artist, t.album_artist, t.album, a.title AS album_title,
                 a.artist_name AS album_artist_name, t.duration_seconds
             FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
             WHERE t.missing_since IS NULL AND a.missing_since IS NULL",
        )
        .fetch_all(pool)
        .await?;
        let mut by_prefix: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
        for row in rows {
            let mut artists: Vec<&str> = Vec::new();
            if let Some(artist) = present(&row.artist) {
                artists.push(artist);
                if let Some(first) = artist.split([';', '/', '|', '\0']).next() {
                    artists.push(first);
                }
            }
            artists.extend(present(&row.album_artist));
            artists.push(&row.album_artist_name);
            if present(&row.artist).is_none() && present(&row.album_artist).is_none() {
                artists.push("Unknown Artist");
            }
            let mut albums: Vec<&str> = present(&row.album).into_iter().collect();
            albums.push(&row.album_title);
            let ms = row
                .duration_seconds
                .map_or(0, |s| (s * 1000.0).round() as i64);
            let seconds = (ms + 500) / 1000;
            let title = normalise(&row.title);
            let mut prefixes: Vec<String> = artists
                .iter()
                .flat_map(|artist| {
                    albums
                        .iter()
                        .map(|album| format!("{}|{}|{title}", normalise(artist), normalise(album)))
                })
                .collect();
            prefixes.sort_unstable();
            prefixes.dedup();
            for prefix in prefixes {
                by_prefix.entry(prefix).or_default().push((seconds, row.id));
            }
        }
        Ok(Self { by_prefix })
    }

    pub(super) fn find(&self, content_key: &str) -> Found {
        let Some((prefix, seconds)) = content_key.rsplit_once('|') else {
            return Found::Missing;
        };
        let (Some(candidates), Ok(seconds)) = (self.by_prefix.get(prefix), seconds.parse::<i64>())
        else {
            return Found::Missing;
        };
        let within = |slack: i64| {
            let mut ids: Vec<i64> = candidates
                .iter()
                .filter(|(s, _)| (s - seconds).abs() <= slack)
                .map(|&(_, id)| id)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        match within(0).as_slice() {
            [id] => return Found::Exact(*id),
            [] => {}
            _ => return Found::Ambiguous,
        }
        match within(1).as_slice() {
            [id] => Found::Near(*id),
            [] => Found::Missing,
            _ => Found::Ambiguous,
        }
    }
}
