//! Finding by words, as someone types them: each word matched on its own, loosely (a typo, a
//! missing colon or a word left out still finds it), with the words as one phrase counting
//! most. Each result keeps MusicBrainz's score. Matching a file to a release still asks for the
//! exact phrase (`search`, `search_songs`).

use super::*;

/// Words of a search at most; more are left out.
const MOST_WORDS: usize = 8;
/// Words this long or longer are matched loosely; shorter ones as they are, since a loose
/// match on two letters matches nearly anything.
const LOOSE_FROM: usize = 4;

/// A result with how well it matched, 0 to 100.
#[derive(Deserialize)]
struct Scored<T> {
    #[serde(default)]
    score: i64,
    #[serde(flatten)]
    item: T,
}

#[derive(Deserialize)]
struct Artists {
    #[serde(default)]
    artists: Vec<Scored<FoundArtist>>,
}

#[derive(Deserialize)]
struct Groups {
    #[serde(rename = "release-groups", default)]
    release_groups: Vec<Scored<ReleaseGroupDetail>>,
}

#[derive(Deserialize)]
struct Recordings {
    #[serde(default)]
    recordings: Vec<Scored<RecordingDetail>>,
}

/// The words of `text`, lower case (so none reads as an operator), letters and digits only
/// (so none can change the query).
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(MOST_WORDS)
        .map(str::to_lowercase)
        .collect()
}

/// A Lucene query for `text` in `field`: the phrase, boosted most; every word, each loosely;
/// any word. None when it has no words.
fn words_query(field: &str, text: &str) -> Option<String> {
    let words = words(text);
    if words.is_empty() {
        return None;
    }
    let loose: Vec<String> = words
        .iter()
        .map(|w| {
            if w.chars().count() >= LOOSE_FROM {
                format!("{w}~")
            } else {
                w.clone()
            }
        })
        .collect();
    Some(format!(
        "{field}:{}^4 OR {field}:({})^2 OR {field}:({})",
        phrase(&words.join(" ")),
        loose.join(" AND "),
        loose.join(" OR ")
    ))
}

/// The words in `field`, and when an artist is given, their words in `artist` too.
fn narrowed(field: &str, text: &str, artist: Option<&str>) -> Option<String> {
    let query = words_query(field, text)?;
    Some(match artist.and_then(|a| words_query("artist", a)) {
        Some(by) => format!("({query}) AND ({by})"),
        None => query,
    })
}

impl Client {
    /// Artists whose names have these words, each with its score.
    pub async fn find_artists(&self, text: &str) -> Result<Vec<(i64, FoundArtist)>, Error> {
        let Some(query) = words_query("artist", text) else {
            return Ok(Vec::new());
        };
        let found: Artists = self.find("artist", query).await?;
        Ok(found
            .artists
            .into_iter()
            .map(|s| (s.score, s.item))
            .collect())
    }

    /// Albums (release groups) whose titles have these words, by an artist with those words
    /// when given, each with its score.
    pub async fn find_albums(
        &self,
        text: &str,
        artist: Option<&str>,
    ) -> Result<Vec<(i64, ReleaseGroupDetail)>, Error> {
        let Some(query) = narrowed("releasegroup", text, artist) else {
            return Ok(Vec::new());
        };
        let found: Groups = self.find("release-group", query).await?;
        Ok(found
            .release_groups
            .into_iter()
            .map(|s| (s.score, s.item))
            .collect())
    }

    /// Songs (recordings) whose titles have these words, by an artist with those words when
    /// given, each with its score.
    pub async fn find_songs(
        &self,
        text: &str,
        artist: Option<&str>,
    ) -> Result<Vec<(i64, RecordingDetail)>, Error> {
        let Some(query) = narrowed("recording", text, artist) else {
            return Ok(Vec::new());
        };
        let found: Recordings = self.find("recording", query).await?;
        Ok(found
            .recordings
            .into_iter()
            .map(|s| (s.score, s.item))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_matched_one_by_one_and_loosely_with_the_phrase_first() {
        assert_eq!(
            words_query("releasegroup", "OK Computr").unwrap(),
            r#"releasegroup:"ok computr"^4 OR releasegroup:(ok AND computr~)^2 OR releasegroup:(ok OR computr~)"#
        );
        // Punctuation and operators can't change the query: they're not words.
        assert_eq!(
            words_query("artist", r#"AC/DC: "Back" (in) NOT Black*"#).unwrap(),
            r#"artist:"ac dc back in not black"^4 OR artist:(ac AND dc AND back~ AND in AND not AND black~)^2 OR artist:(ac OR dc OR back~ OR in OR not OR black~)"#
        );
        assert_eq!(words_query("artist", " ?! "), None);
        assert_eq!(
            words("one two three four five six seven eight nine ten").len(),
            MOST_WORDS
        );
        // Letters of every script are words.
        assert_eq!(
            words("Sigur Rós – Ágætis byrjun"),
            ["sigur", "rós", "ágætis", "byrjun"]
        );
    }

    #[tokio::test]
    async fn finds_keep_musicbrainzs_scores() {
        let (client, log) = testing::start(0).await;
        let found = client.find_artists("radiohed").await.unwrap();
        assert_eq!(found[0].1.name, "Radiohead");
        assert!(
            found[0].0 > found[1].0,
            "{:?}",
            found.iter().map(|f| f.0).collect::<Vec<_>>()
        );
        let albums = client.find_albums("ok computer", None).await.unwrap();
        assert!(!albums.is_empty());
        let songs = client
            .find_songs("paranoid", Some("radiohead"))
            .await
            .unwrap();
        assert_eq!(songs[0].1.title, "Paranoid Android");
        let log = log.lock().unwrap().join("\n");
        // Only the words, as the query: "radiohed" loosely.
        assert!(log.contains("artist%3A%28radiohed%7E%29"), "{log}");
        assert!(log.contains("%29+AND+%28artist%3A%22radiohead%22"), "{log}");
        assert!(client.find_songs("--", None).await.unwrap().is_empty());
    }
}
