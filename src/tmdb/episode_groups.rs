//! A series' episode groups: TMDB's other orders of its episodes (the seasons other sites use,
//! DVD order, story arcs), each a list of parts holding TMDB's own episodes. Only the series'
//! TMDB id and a group's id leave the server.

use super::*;

/// One of a series' episode groups, as TMDB lists them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GroupListed {
    /// TMDB's id for the group.
    pub id: String,
    pub name: String,
    /// TMDB's type: 1 original air date, 2 absolute, 3 DVD, 4 digital, 5 story arc,
    /// 6 production, 7 TV.
    #[serde(rename = "type", default)]
    pub kind: Option<i64>,
}

/// A group's episodes as seasons.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeGroup {
    pub id: String,
    pub name: String,
    pub kind: Option<i64>,
    /// Its seasons: (season, its episodes' TMDB ids in order, the first being episode 1).
    pub seasons: Vec<(i64, Vec<i64>)>,
}

#[derive(Deserialize)]
struct GroupList {
    #[serde(default, deserialize_with = "null_as_default")]
    results: Vec<GroupListed>,
}

#[derive(Deserialize)]
struct GroupDetails {
    id: String,
    name: String,
    #[serde(rename = "type", default)]
    kind: Option<i64>,
    #[serde(default, deserialize_with = "null_as_default")]
    groups: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    #[serde(default)]
    name: String,
    #[serde(default)]
    order: i64,
    #[serde(default, deserialize_with = "null_as_default")]
    episodes: Vec<PartEpisode>,
}

#[derive(Deserialize)]
struct PartEpisode {
    id: i64,
    #[serde(default)]
    order: i64,
}

/// Whether a group id is one TMDB would give: letters and digits only, so it's safe in a path.
pub fn is_group_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl Client {
    /// A series' episode groups; `None` when TMDB has no such series.
    pub async fn episode_groups(&self, series: i64) -> anyhow::Result<Option<Vec<GroupListed>>> {
        let list: Option<GroupList> = self
            .get(&format!("/tv/{series}/episode_groups"), &[])
            .await?;
        Ok(list.map(|l| {
            l.results
                .into_iter()
                .filter(|g| is_group_id(&g.id))
                .collect()
        }))
    }

    /// A group's episodes as seasons; `None` when TMDB has no such group.
    pub async fn episode_group(&self, id: &str) -> anyhow::Result<Option<EpisodeGroup>> {
        if !is_group_id(id) {
            bail!("{id:?} isn't a TMDB episode group id");
        }
        let details: Option<GroupDetails> =
            self.get(&format!("/tv/episode_group/{id}"), &[]).await?;
        Ok(details.map(as_seasons))
    }
}

/// A group's parts as seasons: in TMDB's order, numbered from 1, a part called Specials (or
/// Season 0) being season 0; each part's episodes in their order, numbered from 1. An episode
/// listed twice counts where it's first listed.
fn as_seasons(details: GroupDetails) -> EpisodeGroup {
    let mut parts = details.groups;
    parts.sort_by_key(|p| p.order);
    let mut seen = std::collections::HashSet::new();
    let mut seasons: Vec<(i64, Vec<i64>)> = Vec::new();
    let mut next = 1;
    for mut part in parts {
        let name = part.name.trim().to_lowercase();
        let special = name.starts_with("special") || name == "season 0" || name == "season 00";
        let season = if special {
            0
        } else {
            next += 1;
            next - 1
        };
        part.episodes.sort_by_key(|e| e.order);
        let episodes: Vec<i64> = part
            .episodes
            .iter()
            .map(|e| e.id)
            .filter(|id| seen.insert(*id))
            .collect();
        match seasons.iter_mut().find(|(s, _)| *s == season) {
            // A second specials part follows the first.
            Some((_, known)) => known.extend(episodes),
            None => seasons.push((season, episodes)),
        }
    }
    EpisodeGroup {
        id: details.id,
        name: details.name,
        kind: details.kind,
        seasons,
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use serde_json::json;

    #[tokio::test]
    async fn groups_are_read_as_seasons_in_order() {
        let fake = FakeTmdb::default();
        fake.reply(
            "/tv/7/episode_groups",
            vec![Reply::ok(json!({"id": 7, "results": [
                {"id": "5acf93e60e0a26346d0000ce", "name": "Seasons", "type": 6,
                 "episode_count": 5, "group_count": 3},
                {"id": "../tv/1", "name": "Not an id", "type": 1}
            ]}))],
        );
        fake.reply(
            "/tv/episode_group/5acf93e60e0a26346d0000ce",
            vec![Reply::ok(json!({
                "id": "5acf93e60e0a26346d0000ce", "name": "Seasons", "type": 6,
                "groups": [
                    {"name": "Season 2", "order": 2, "episodes": [
                        {"id": 14, "order": 1}, {"id": 13, "order": 0}]},
                    {"name": "Specials", "order": 0, "episodes": [{"id": 90, "order": 0}]},
                    {"name": "Season 1", "order": 1, "episodes": [
                        {"id": 11, "order": 0}, {"id": 12, "order": 1}, {"id": 11, "order": 2}]}
                ]
            }))],
        );
        let tmdb = fake.start().await;
        let listed = tmdb.episode_groups(7).await.unwrap().unwrap();
        assert_eq!(listed.len(), 1, "an id that isn't one is left out");
        assert_eq!(listed[0].kind, Some(6));
        let group = tmdb
            .episode_group("5acf93e60e0a26346d0000ce")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            group.seasons,
            vec![(0, vec![90]), (1, vec![11, 12]), (2, vec![13, 14])]
        );
        assert!(tmdb.episode_group("../tv/1").await.is_err());
        assert_eq!(tmdb.episode_groups(8).await.unwrap(), None);
        // Only the series' id and the group's went out.
        assert_eq!(
            *fake.log.lock().unwrap(),
            vec![
                "/tv/7/episode_groups",
                "/tv/episode_group/5acf93e60e0a26346d0000ce",
                "/tv/8/episode_groups"
            ]
        );
    }
}
