//! What an admin may choose for a request instead of the defaults: where it goes, its profile, whether it's monitored and searched for at once, and a series' seasons.

use super::*;

/// What an approver (or an admin asking) may choose instead of the defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    /// The library it goes in; `None` for the request defaults' choice.
    pub library_id: Option<i64>,
    /// The quality profile for a movie or series; `None` for the default.
    pub profile_id: Option<i64>,
    /// The music profile for an artist, album or song; `None` for the default.
    pub music_profile_id: Option<i64>,
    /// Whether it's monitored once on the wanted list; `None` for monitored. Not for a song.
    pub monitored: Option<bool>,
    /// Whether it's searched for at once; `None` for yes, as every approval is.
    pub search_now: Option<bool>,
    /// For a series: the seasons monitored, instead of the one asked for or all of them.
    pub seasons: Option<Vec<i64>>,
}

impl Choice {
    /// Whether anything is chosen.
    pub fn chooses(&self) -> bool {
        self.library_id.is_some()
            || self.profile_id.is_some()
            || self.music_profile_id.is_some()
            || self.monitored.is_some()
            || self.search_now.is_some()
            || self.seasons.is_some()
    }

    /// Why the choice doesn't fit a request of this kind, if it doesn't.
    pub fn misfit(&self, kind: &str) -> Option<String> {
        let video = matches!(kind, "movie" | "series");
        if self.profile_id.is_some() && !video {
            return Some(
                "profile_id is for a movie or series; music takes music_profile_id".into(),
            );
        }
        if self.music_profile_id.is_some() && video {
            return Some(
                "music_profile_id is for music; a movie or series takes profile_id".into(),
            );
        }
        if self.seasons.is_some() && kind != "series" {
            return Some("seasons are for a series".into());
        }
        if self
            .seasons
            .as_ref()
            .is_some_and(|s| s.iter().any(|n| *n < 0))
        {
            return Some("a season is 0 or more".into());
        }
        if self.monitored.is_some() && kind == "song" {
            return Some("a song isn't monitored; it's searched for once".into());
        }
        None
    }

    /// Whether the title is searched for once it's approved.
    pub fn searches_now(&self) -> bool {
        self.search_now != Some(false)
    }

    /// Why the chosen library or profile can't take a request of this kind, if it can't: a
    /// library of the title's kind, a profile for its kind of title.
    pub async fn unusable(&self, pool: &SqlitePool, kind: &str) -> anyhow::Result<Option<String>> {
        let (library_kind, source) = match kind {
            "movie" => ("movies", "radarr"),
            "series" => ("shows", "sonarr"),
            _ => ("music", ""),
        };
        if let Some(id) = self.library_id {
            let found: Option<String> =
                sqlx::query_scalar("SELECT kind FROM libraries WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if found.as_deref() != Some(library_kind) {
                return Ok(Some(format!("library {id} isn't a {library_kind} library")));
            }
        }
        let profile = match (self.profile_id, self.music_profile_id) {
            (Some(id), _) => Some((id, "SELECT source = ?2 FROM quality_profiles WHERE id = ?1")),
            (_, Some(id)) => Some((id, "SELECT ?2 = '' FROM music_profiles WHERE id = ?1")),
            _ => None,
        };
        if let Some((id, query)) = profile {
            let fits: Option<bool> = sqlx::query_scalar(query)
                .bind(id)
                .bind(source)
                .fetch_optional(pool)
                .await?;
            if fits != Some(true) {
                return Ok(Some(format!("profile {id} isn't one for a {kind}")));
            }
        }
        Ok(None)
    }
}

/// Why what a request chooses is refused, if it is: anyone but an admin choosing, or a choice
/// that doesn't fit the kind asked for. Only kinds requests take are checked.
pub(super) async fn refuse_choice(
    pool: &SqlitePool,
    user: &User,
    kind: &str,
    choice: &Choice,
) -> anyhow::Result<Option<String>> {
    if !choice.chooses() || !matches!(kind, "movie" | "series" | "artist" | "album" | "song") {
        return Ok(None);
    }
    if !user.is_admin {
        return Ok(Some(
            "only an admin chooses where a request goes and how it's looked for".into(),
        ));
    }
    if let Some(why) = choice.misfit(kind) {
        return Ok(Some(why));
    }
    choice.unusable(pool, kind).await
}
