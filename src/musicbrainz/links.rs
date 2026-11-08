//! An artist's links to other sites, as MusicBrainz's url relations give them: only its
//! Wikidata item is read, for artists' portraits (`artist_portraits`).

use super::*;

#[derive(Deserialize)]
struct WithRelations {
    #[serde(default)]
    relations: Vec<Relation>,
}

#[derive(Deserialize)]
struct Relation {
    #[serde(rename = "type", default)]
    kind: String,
    url: Option<Linked>,
}

#[derive(Deserialize)]
struct Linked {
    resource: String,
}

impl Client {
    /// An artist's Wikidata item (`Q44190`), from its url relations; none when it has no link
    /// there, or the link isn't to an item on wikidata.org.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the id isn't a MusicBrainz id or MusicBrainz has no such artist;
    /// [`Error::Failed`] when the request or its answer fails.
    pub async fn artist_wikidata(&self, id: &str) -> Result<Option<String>, Error> {
        if !crate::library::tags::is_mbid(id) {
            return Err(Error::NotFound);
        }
        let body = self.lookup("artist", id, "inc=url-rels&fmt=json").await?;
        let artist: WithRelations = serde_json::from_str(&body)
            .map_err(|err| Error::Failed(format!("its answer: {err}")))?;
        Ok(artist
            .relations
            .into_iter()
            .filter(|r| r.kind == "wikidata")
            .filter_map(|r| r.url)
            .find_map(|u| {
                let item = u.resource.strip_prefix("https://www.wikidata.org/wiki/")?;
                crate::wikimedia::is_item(item).then(|| item.to_owned())
            }))
    }
}

/// The fake MusicBrainz's answer for an artist looked up with its url relations: Radiohead's
/// Wikidata item (Q44190) and homepage; none for the tribute act; others unknown.
#[cfg(test)]
pub(super) fn fake_url_relations(id: &str) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let relations = match id {
        testing::RADIOHEAD => serde_json::json!([
            {"type": "official homepage", "url": {"resource": "https://www.radiohead.com/"}},
            {"type": "wikidata", "url": {"resource": "https://www.wikidata.org/wiki/Q44190"}},
        ]),
        testing::TRIBUTE_ACT => serde_json::json!([]),
        _ => return (StatusCode::NOT_FOUND, r#"{"error":"Not Found"}"#).into_response(),
    };
    let body = serde_json::json!({"id": id, "name": "Radiohead", "relations": relations});
    (StatusCode::OK, body.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_an_artists_wikidata_item_from_its_url_relations() {
        let (client, log) = testing::start(0).await;
        let item = client.artist_wikidata(testing::RADIOHEAD).await.unwrap();
        assert_eq!(item.as_deref(), Some("Q44190"));
        let none = client.artist_wikidata(testing::TRIBUTE_ACT).await.unwrap();
        assert_eq!(none, None, "no Wikidata link");
        assert!(matches!(
            client.artist_wikidata("not-an-id").await,
            Err(Error::NotFound)
        ));
        assert_eq!(
            *log.lock().unwrap(),
            [
                format!("artist {}", testing::RADIOHEAD),
                format!("artist {}", testing::TRIBUTE_ACT)
            ],
            "only the artist's id"
        );
    }
}
