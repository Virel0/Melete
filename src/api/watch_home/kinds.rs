//! The Watch pages' kinds, for a home narrowed to one (`/home?kind=`): movies, shows, or anime,
//! a series being anime when `/series?anime=only` lists it (wanted as anime, or in an anime
//! library) and a show otherwise.

use super::*;

/// One of Watch's pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Kind {
    Movies,
    Shows,
    Anime,
}

/// Which kind each title is, for the titles it was made for.
pub(super) struct Kinds {
    anime: HashSet<i64>,
}

impl Kinds {
    /// The kinds of `titles`: which of the series among them are anime.
    pub(super) async fn of(
        pool: &SqlitePool,
        titles: impl IntoIterator<Item = TitleRef>,
    ) -> Result<Self, ApiError> {
        let series: Vec<i64> = titles
            .into_iter()
            .filter(|t| t.series)
            .map(|t| t.id)
            .collect();
        let anime: Vec<i64> = if series.is_empty() {
            Vec::new()
        } else {
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT s.id FROM series s JOIN libraries l ON l.id = s.library_id
                 WHERE s.id IN (SELECT value FROM json_each(?)) AND {}",
                anime_sql("s", "l")
            )))
            .bind(serde_json::to_string(&series).unwrap_or_default())
            .fetch_all(pool)
            .await?
        };
        Ok(Self {
            anime: anime.into_iter().collect(),
        })
    }

    /// The kind `title` is.
    pub(super) fn of_title(&self, title: TitleRef) -> Kind {
        match (title.series, self.anime.contains(&title.id)) {
            (false, _) => Kind::Movies,
            (true, true) => Kind::Anime,
            (true, false) => Kind::Shows,
        }
    }

    /// Whether `title` is `kind`; always, without one.
    pub(super) fn is(&self, kind: Option<Kind>, title: TitleRef) -> bool {
        kind.is_none_or(|k| self.of_title(title) == k)
    }
}
