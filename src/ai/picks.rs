//! Picks: titles chosen from a list of candidates, for one viewer from what they've watched, or
//! like one title. What goes to OpenRouter is only what [`Told`] holds of each title: its title,
//! year, up to three genres and a premise cut to about 12 words; the candidates go by an opaque
//! id that means nothing outside this request. Never who's asking or anything about the
//! household, and nothing else about the library.

use super::*;

/// The most genres told about a title.
pub const MAX_GENRES: usize = 3;
/// About how many words of a title's premise are told.
pub const PREMISE_WORDS: usize = 12;
/// The longest reason kept, in characters.
const MAX_REASON: usize = 80;

/// A title as the model is told about it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Told {
    /// An id for the candidates (`c1`, `c2`…), meaningless outside the request; none for a
    /// title watched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The title.
    pub title: String,
    /// The year it came out, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    /// Up to [`MAX_GENRES`] genres.
    pub genres: Vec<String>,
    /// Its premise, cut to about [`PREMISE_WORDS`] words, when one is told.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premise: Option<String>,
}

impl Told {
    /// A title as told: genres cut to [`MAX_GENRES`], and the premise to about
    /// [`PREMISE_WORDS`] words.
    pub fn new(
        id: Option<String>,
        title: &str,
        year: Option<i64>,
        genres: &[String],
        overview: Option<&str>,
    ) -> Told {
        Told {
            premise: overview.map(premise).filter(|p| !p.is_empty()),
            id,
            title: title.to_owned(),
            year,
            genres: genres.iter().take(MAX_GENRES).cloned().collect(),
        }
    }
}

/// The first [`PREMISE_WORDS`] words of an overview, with "…" when there were more.
pub fn premise(overview: &str) -> String {
    let words: Vec<&str> = overview.split_whitespace().collect();
    let mut cut = words
        .iter()
        .take(PREMISE_WORDS)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if words.len() > PREMISE_WORDS {
        cut.push('…');
    }
    cut
}

/// A candidate the model picked, and why, in a few words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// The candidate's id, as it was told.
    pub id: String,
    /// Why, in a few words, as the model put it.
    pub reason: String,
}

impl Client {
    /// Up to `count` of `candidates` for someone who watched `watched`, best first.
    pub async fn pick(
        &self,
        watched: &[Told],
        candidates: &[Told],
        count: usize,
    ) -> Result<Vec<Picked>, Failure> {
        let system = format!(
            "You choose what someone should watch next. You're given titles they watched \
             recently and a list of candidates, each with an id. Pick up to {count} candidates \
             they'd most enjoy, best first, only from the list. Answer with JSON only, no other \
             text: {{\"picks\": [{{\"id\": \"c1\", \"reason\": \"...\"}}]}}, each reason at \
             most 8 words, such as \"Like Heat, a tense crime thriller\"."
        );
        let told = json!({ "watched": watched, "candidates": candidates });
        self.picked(&system, &told, candidates, count).await
    }

    /// Up to `count` of `candidates` most like `title`, best first.
    pub async fn similar(
        &self,
        title: &Told,
        candidates: &[Told],
        count: usize,
    ) -> Result<Vec<Picked>, Failure> {
        let system = format!(
            "You find titles like a given one. You're given a title and a list of candidates, \
             each with an id. Pick up to {count} candidates most like it, best first, only \
             from the list. Answer with JSON only, no other text: {{\"picks\": [{{\"id\": \
             \"c1\", \"reason\": \"...\"}}]}}, each reason at most 8 words."
        );
        let told = json!({ "title": title, "candidates": candidates });
        self.picked(&system, &told, candidates, count).await
    }

    async fn picked(
        &self,
        system: &str,
        told: &Value,
        candidates: &[Told],
        count: usize,
    ) -> Result<Vec<Picked>, Failure> {
        let answer = self.complete(system, &told.to_string()).await?;
        parse_picks(&answer, candidates, count).ok_or_else(|| {
            Failure::Other("the model's answer wasn't the list asked for".to_owned())
        })
    }
}

/// The picks in a model's answer: ids of `candidates` only, each once, at most `count`, with a
/// reason of sensible length. None when there's no such list.
fn parse_picks(answer: &str, candidates: &[Told], count: usize) -> Option<Vec<Picked>> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    let value: Value = serde_json::from_str(answer.get(start..=end)?).ok()?;
    let picks = value["picks"].as_array()?;
    let mut seen = std::collections::HashSet::new();
    Some(
        picks
            .iter()
            .filter_map(|p| {
                let id = p["id"].as_str()?.trim();
                let known = candidates.iter().any(|c| c.id.as_deref() == Some(id));
                if !known || !seen.insert(id.to_owned()) {
                    return None;
                }
                let reason: String = p["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_REASON)
                    .collect();
                Some(Picked {
                    id: id.to_owned(),
                    reason,
                })
            })
            .take(count)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, title: &str) -> Told {
        Told::new(
            Some(id.into()),
            title,
            Some(2001),
            &["Drama".into()],
            Some("A long story told over many more words than twelve, so it gets cut short."),
        )
    }

    #[test]
    fn a_title_is_told_by_its_title_year_three_genres_and_a_short_premise() {
        let genres: Vec<String> = ["A", "B", "C", "D"].map(String::from).to_vec();
        let told = Told::new(
            Some("c1".into()),
            "X",
            Some(1999),
            &genres,
            Some("One two."),
        );
        assert_eq!(
            serde_json::to_value(&told).unwrap(),
            json!({ "id": "c1", "title": "X", "year": 1999, "genres": ["A", "B", "C"],
                    "premise": "One two." })
        );
        assert_eq!(
            candidate("c2", "Y").premise.unwrap(),
            "A long story told over many more words than twelve, so it…"
        );
        // A title watched is told without an id or a premise.
        let watched = Told::new(None, "Z", None, &genres[..1], None);
        assert_eq!(
            serde_json::to_value(&watched).unwrap(),
            json!({ "title": "Z", "genres": ["A"] })
        );
    }

    #[test]
    fn picks_are_candidates_only_each_once() {
        let candidates = [candidate("c1", "A"), candidate("c2", "B")];
        let answer = r#"Sure: {"picks": [{"id": "c2", "reason": " Like A, but sadder "},
            {"id": "c9", "reason": "made up"}, {"id": "c2", "reason": "again"},
            {"id": "c1"}]}"#;
        assert_eq!(
            parse_picks(answer, &candidates, 5).unwrap(),
            [
                Picked {
                    id: "c2".into(),
                    reason: "Like A, but sadder".into()
                },
                Picked {
                    id: "c1".into(),
                    reason: String::new()
                },
            ]
        );
        assert_eq!(parse_picks(answer, &candidates, 1).unwrap().len(), 1);
        assert!(parse_picks("no", &candidates, 5).is_none());
    }
}
