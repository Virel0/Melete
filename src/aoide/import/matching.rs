//! Finding a record's track in Melete's library by the content key the apps built from its tags.
//!
//! A key is `artist|album|title|seconds`. It's looked for as it is first, the length the same or
//! a second either side. A track credited to several artists is found by any of them: the parts
//! of its artist tags, and the artists Melete credits it to, each stand for it, and so does each
//! artist the key names when it names several ("A, B", "A & B", "A feat. B"). When the key's
//! album fits none of the artist's tracks of that title (another edition's name, say), those
//! tracks on other albums are looked at: one within a second is taken; of several, the one whose
//! album name holds the key's or is held in it, else the one of exactly its length. Anything
//! still tied is ambiguous, never guessed.

use super::*;

#[derive(sqlx::FromRow)]
struct TrackRow {
    id: i64,
    title: String,
    artist: Option<String>,
    album_artist: Option<String>,
    album: Option<String>,
    album_title: String,
    album_artist_name: String,
    duration_seconds: Option<f64>,
    /// The artists Melete credits the track to, joined by NULs.
    credited: Option<String>,
}

/// One track under a key: its length in whole seconds, its id, and its album names.
type Entry = (i64, i64, Vec<String>);

/// Present tracks under every `artist|album|title` the apps could have built for them from
/// Jellyfin's view of the same tags (each credited artist, else the album artist, else "Unknown
/// Artist"; the album tag or the album's title), and under every `artist|title` for keys whose
/// album fits none.
pub(super) struct Library {
    by_prefix: HashMap<String, Vec<Entry>>,
    by_artist_title: HashMap<String, Vec<Entry>>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Found {
    Exact(i64),
    /// A second either side (Jellyfin and Melete can round a duration differently), or found on
    /// another album.
    Near(i64),
    Ambiguous,
    Missing,
}

/// The artists a name credits: the whole name, then each artist it lists (split on `;`, `/`,
/// `|`, NULs and commas, and on " & ", " feat. ", " ft. " and " featuring ").
fn credited(name: &str) -> Vec<&str> {
    let mut parts = vec![name];
    for joiner in [" & ", " feat. ", " ft. ", " featuring "] {
        parts = parts.iter().flat_map(|p| p.split(joiner)).collect();
    }
    let mut names = vec![name.trim()];
    names.extend(
        parts
            .iter()
            .flat_map(|p| p.split([';', '/', '|', '\0', ',']))
            .map(str::trim),
    );
    names.retain(|n| !n.is_empty());
    names.dedup();
    names
}

impl Library {
    pub(super) async fn load(pool: &SqlitePool) -> sqlx::Result<Self> {
        let rows: Vec<TrackRow> = sqlx::query_as(
            "SELECT t.id, t.title, t.artist, t.album_artist, t.album, a.title AS album_title,
                 a.artist_name AS album_artist_name, t.duration_seconds,
                 (SELECT group_concat(ta.name, char(0)) FROM music_track_artists ta
                  WHERE ta.track_id = t.id) AS credited
             FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
             WHERE t.missing_since IS NULL AND a.missing_since IS NULL",
        )
        .fetch_all(pool)
        .await?;
        let mut library = Self {
            by_prefix: HashMap::new(),
            by_artist_title: HashMap::new(),
        };
        for row in &rows {
            library.add(row);
        }
        Ok(library)
    }

    fn add(&mut self, row: &TrackRow) {
        let mut artists: Vec<&str> = Vec::new();
        for tag in [present(&row.artist), row.credited.as_deref()]
            .into_iter()
            .flatten()
        {
            artists.extend(credited(tag));
        }
        if let Some(tag) = present(&row.album_artist) {
            artists.extend(credited(tag));
        }
        artists.push(&row.album_artist_name);
        if present(&row.artist).is_none() && present(&row.album_artist).is_none() {
            artists.push("Unknown Artist");
        }
        let mut artists: Vec<String> = artists.into_iter().map(normalise).collect();
        artists.sort_unstable();
        artists.dedup();
        let mut albums: Vec<String> = present(&row.album)
            .into_iter()
            .chain([row.album_title.as_str()])
            .map(normalise)
            .collect();
        albums.dedup();
        let ms = row
            .duration_seconds
            .map_or(0, |s| (s * 1000.0).round() as i64);
        let entry: Entry = ((ms + 500) / 1000, row.id, albums.clone());
        let title = normalise(&row.title);
        for artist in &artists {
            for album in &albums {
                let key = format!("{artist}|{album}|{title}");
                self.by_prefix.entry(key).or_default().push(entry.clone());
            }
            let key = format!("{artist}|{title}");
            self.by_artist_title
                .entry(key)
                .or_default()
                .push(entry.clone());
        }
    }

    pub(super) fn find(&self, content_key: &str) -> Found {
        let Some((prefix, seconds)) = content_key.rsplit_once('|') else {
            return Found::Missing;
        };
        let Ok(seconds) = seconds.parse::<i64>() else {
            return Found::Missing;
        };
        let found = within(self.by_prefix.get(prefix).into_iter().flatten(), seconds);
        if found != Found::Missing {
            return found;
        }
        // The artist before the first bar, the title after the last, the album between.
        let (Some((artist, rest)), Some((_, title))) =
            (prefix.split_once('|'), prefix.rsplit_once('|'))
        else {
            return Found::Missing;
        };
        let album = rest.rsplit_once('|').map_or("", |(album, _)| album);
        let names = credited(artist);
        let under = |index: &HashMap<String, Vec<Entry>>, key: &dyn Fn(&str) -> String| {
            let mut entries: Vec<Entry> = names
                .iter()
                .filter_map(|n| index.get(&key(n)))
                .flatten()
                .cloned()
                .collect();
            entries.sort_unstable_by_key(|(_, id, _)| *id);
            entries.dedup_by_key(|(_, id, _)| *id);
            entries
        };
        // Each artist the key names, on the key's album.
        let on_album = under(&self.by_prefix, &|n| format!("{n}|{album}|{title}"));
        let found = within(on_album.iter(), seconds);
        if found != Found::Missing {
            return found;
        }
        let elsewhere = under(&self.by_artist_title, &|n| format!("{n}|{title}"));
        on_other_albums(&elsewhere, album, seconds)
    }
}

/// Of `entries`, the one track of `seconds`, else the one a second either side.
fn within<'a>(entries: impl Iterator<Item = &'a Entry> + Clone, seconds: i64) -> Found {
    let ids = |slack: i64| {
        let mut ids: Vec<i64> = entries
            .clone()
            .filter(|(s, _, _)| (s - seconds).abs() <= slack)
            .map(|&(_, id, _)| id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    match ids(0).as_slice() {
        [id] => return Found::Exact(*id),
        [] => {}
        _ => return Found::Ambiguous,
    }
    match ids(1).as_slice() {
        [id] => Found::Near(*id),
        [] => Found::Missing,
        _ => Found::Ambiguous,
    }
}

/// Of `entries` (the tracks of the key's artist and title, whatever their album, each once),
/// those within a second of `seconds`: the one, else the one whose album name holds `album` or
/// is held in it, else the one of exactly `seconds`.
fn on_other_albums(entries: &[Entry], album: &str, seconds: i64) -> Found {
    let near: Vec<&Entry> = entries
        .iter()
        .filter(|(s, _, _)| (s - seconds).abs() <= 1)
        .collect();
    let one = |keep: &dyn Fn(&Entry) -> bool| match near
        .iter()
        .filter(|e| keep(e))
        .collect::<Vec<_>>()
        .as_slice()
    {
        [(_, id, _)] => Some(*id),
        _ => None,
    };
    if near.is_empty() {
        return Found::Missing;
    }
    let related =
        |a: &String| !album.is_empty() && (a.contains(album) || album.contains(a.as_str()));
    one(&|_| true)
        .or_else(|| one(&|(_, _, albums)| albums.iter().any(related)))
        .or_else(|| one(&|(s, _, _)| *s == seconds))
        .map_or(Found::Ambiguous, Found::Near)
}

#[cfg(test)]
mod tests;
