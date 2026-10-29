//! Torrent indexers: imported from Prowlarr, each reached through Prowlarr's own
//! Torznab feed for it, with Prowlarr's API key, or Melete's own: The Pirate Bay, Nyaa
//! and AnimeTosho, reached directly. A key is stored (searches need it) and never leaves this
//! module except to `torznab`.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::arr::{ArrClient, ProwlarrCategory};
use crate::db::unix_now;
use crate::import::RefusedToEmpty;
use crate::torznab;

/// What an import from Prowlarr did.
#[derive(Debug, Serialize)]
pub struct ProwlarrReport {
    /// Torrent indexers Prowlarr listed, all now in Melete.
    pub indexers: usize,
    /// Usenet indexers aren't imported: there's no Usenet download client.
    pub skipped_usenet: usize,
    /// Indexers imported before that Prowlarr no longer has.
    pub removed: u64,
}

/// Makes Melete's indexers from Prowlarr match Prowlarr's torrent indexers, turned off ones
/// included (and kept off). A name given and a switch turned off in Melete are kept.
///
/// # Errors
///
/// A [`RefusedToEmpty`] when Prowlarr lists no torrent indexers but some were imported
/// before: nothing is removed then. Otherwise Prowlarr or the database failing.
pub async fn import_prowlarr(
    pool: &SqlitePool,
    client: &ArrClient,
) -> anyhow::Result<ProwlarrReport> {
    client.expect_app("Prowlarr").await?;
    let listed = client.prowlarr_indexers().await?;
    let (torrent, usenet): (Vec<_>, Vec<_>) =
        listed.into_iter().partition(|i| i.protocol == "torrent");
    let now = unix_now();
    let mut tx = pool.begin().await?;
    if torrent.is_empty() {
        let existing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM indexers WHERE source = 'prowlarr'")
                .fetch_one(&mut *tx)
                .await?;
        if existing > 0 {
            return Err(RefusedToEmpty(format!(
                "Prowlarr listed no torrent indexers; refusing to remove all {existing} imported from it (check the URL)"
            ))
            .into());
        }
    }
    let mut kept = Vec::new();
    for indexer in &torrent {
        let mut categories = Vec::new();
        flatten(&indexer.capabilities.categories, &mut categories);
        categories.sort_unstable();
        categories.dedup();
        // Prowlarr's API writes "imdbId"; Torznab's parameter is "imdbid".
        let lower = |params: &[String]| -> Vec<String> {
            params.iter().map(|p| p.to_ascii_lowercase()).collect()
        };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
                 tv_search_params, source, source_id, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'prowlarr', ?, ?)
             ON CONFLICT (source, source_id) DO UPDATE
             SET name = CASE WHEN indexers.renamed THEN indexers.name ELSE excluded.name END,
                 url = excluded.url, api_key = excluded.api_key,
                 enabled = excluded.enabled, categories = excluded.categories,
                 movie_search_params = excluded.movie_search_params,
                 tv_search_params = excluded.tv_search_params
             RETURNING id",
        )
        .bind(&indexer.name)
        .bind(format!("{}/{}/api", client.base(), indexer.id))
        .bind(client.key())
        .bind(indexer.enable)
        .bind(serde_json::to_string(&categories)?)
        .bind(serde_json::to_string(&lower(
            &indexer.capabilities.movie_search_params,
        ))?)
        .bind(serde_json::to_string(&lower(
            &indexer.capabilities.tv_search_params,
        ))?)
        .bind(indexer.id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        kept.push(id);
    }
    let removed = sqlx::query(
        "DELETE FROM indexers WHERE source = 'prowlarr' AND id NOT IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(&kept)?)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(ProwlarrReport {
        indexers: torrent.len(),
        skipped_usenet: usenet.len(),
        removed,
    })
}

/// Standard category ids; Prowlarr's own (100000 and up) are left out.
fn flatten(categories: &[ProwlarrCategory], into: &mut Vec<i64>) {
    for category in categories {
        if category.id < 100_000 {
            into.push(category.id);
        }
        flatten(&category.sub_categories, into);
    }
}

/// An indexer as the API shows it: without its key.
#[derive(Debug, Serialize)]
pub struct Indexer {
    /// The indexer's row id.
    pub id: i64,
    /// Its name, Prowlarr's or the one given in Melete.
    pub name: String,
    /// Where it's reached: Prowlarr's feed for it, or the tracker's own address. Never holds
    /// the key.
    pub url: String,
    /// Whether searches use it: on in its source and not turned off in Melete.
    pub enabled: bool,
    /// The Torznab categories it has (2000 movies, 5000 TV and so on).
    pub categories: Vec<i64>,
    /// The parameters its movie searches take ("q", "imdbid"…); empty when it has none.
    pub movie_search_params: Vec<String>,
    /// The parameters its TV searches take ("q", "tvdbid", "season"…).
    pub tv_search_params: Vec<String>,
    /// Where it came from: "prowlarr", or "melete" for a preset or one added by hand.
    pub source: String,
    /// How it's reached: torznab, apibay (The Pirate Bay) or nyaa.
    pub kind: String,
    /// When it was first added, in Unix seconds.
    pub added_at: i64,
}

#[derive(sqlx::FromRow)]
struct IndexerRow {
    id: i64,
    name: String,
    url: String,
    enabled: bool,
    categories: String,
    movie_search_params: String,
    tv_search_params: String,
    source: String,
    kind: String,
    added_at: i64,
}

/// Every indexer, by name, without keys.
pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<Indexer>> {
    let rows: Vec<IndexerRow> = sqlx::query_as(
        "SELECT id, name, url, enabled AND NOT turned_off AS enabled, categories,
                movie_search_params, tv_search_params, source, kind, added_at
         FROM indexers ORDER BY name, id",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Indexer {
                id: row.id,
                name: row.name,
                url: row.url,
                enabled: row.enabled,
                categories: serde_json::from_str(&row.categories)?,
                movie_search_params: serde_json::from_str(&row.movie_search_params)?,
                tv_search_params: serde_json::from_str(&row.tv_search_params)?,
                source: row.source,
                kind: row.kind,
                added_at: row.added_at,
            })
        })
        .collect()
}

/// Why an indexer wasn't changed.
#[derive(Debug, PartialEq, Eq)]
pub enum NotChanged {
    /// There's no indexer with that id.
    NotFound,
    /// The name is empty or too long.
    BadName,
    /// Asked to be turned on while its source has it off (Prowlarr): searches would fail.
    OffInSource,
}

/// The longest name an indexer can be given, in characters.
const NAME_MAX: usize = 100;

/// Renames an indexer and turns it on or off in Melete. Both outlast Prowlarr imports.
pub async fn update(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    enabled: bool,
) -> anyhow::Result<Result<(), NotChanged>> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Ok(Err(NotChanged::BadName));
    }
    let mut tx = pool.begin().await?;
    let row: Option<(String, bool)> =
        sqlx::query_as("SELECT name, enabled FROM indexers WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((current, on_in_source)) = row else {
        return Ok(Err(NotChanged::NotFound));
    };
    if enabled && !on_in_source {
        return Ok(Err(NotChanged::OffInSource));
    }
    sqlx::query(
        "UPDATE indexers SET name = ?, turned_off = ?, renamed = renamed OR ? WHERE id = ?",
    )
    .bind(name)
    .bind(!enabled)
    .bind(name != current)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// Removes an indexer; false when there's no such indexer. Searches it answered keep their
/// results (no longer linked to it) and its name in what they asked; RSS forgets what it saw
/// there. One imported from Prowlarr comes back with the next import.
pub async fn remove(pool: &SqlitePool, id: i64) -> anyhow::Result<bool> {
    let removed = sqlx::query("DELETE FROM indexers WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(removed > 0)
}

/// What a test of an indexer found.
#[derive(Debug, Serialize)]
pub struct TestReport {
    /// The categories it has: what a Torznab feed says it can do, or what Melete knows of the
    /// others.
    pub categories: Vec<i64>,
    /// How many releases its newest-releases feed listed.
    pub recent_releases: usize,
}

/// Asks the indexer what it can do and for its newest releases. `None` when there's no such
/// indexer.
pub async fn test(
    pool: &SqlitePool,
    client: &torznab::Client,
    id: i64,
) -> anyhow::Result<Option<Result<TestReport, torznab::Error>>> {
    let row: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT url, api_key, kind, categories FROM indexers WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((url, api_key, kind, categories)) = row else {
        return Ok(None);
    };
    let endpoint = torznab::Endpoint {
        url: &url,
        api_key: &api_key,
    };
    let kind = torznab::Kind::parse(&kind).unwrap_or(torznab::Kind::Torznab);
    let result = async {
        // Only Torznab feeds say what they can do; Melete knows the others.
        let categories: Vec<i64> = match kind {
            torznab::Kind::Torznab => client.caps(&endpoint).await?.categories,
            _ => serde_json::from_str(&categories).unwrap_or_default(),
        };
        let cat: Vec<String> = categories.iter().map(i64::to_string).collect();
        let cat = cat.join(",");
        let recent = client
            .search_as(kind, &endpoint, &[("t", "search"), ("cat", &cat)])
            .await?;
        Ok(TestReport {
            categories,
            recent_releases: recent.len(),
        })
    }
    .await;
    Ok(Some(result))
}

/// A public tracker Melete reaches without Prowlarr.
pub struct Preset {
    /// The name the API adds it by ("thepiratebay", "nyaa"…).
    pub key: &'static str,
    /// The name it's shown and added with.
    pub name: &'static str,
    kind: &'static str,
    url: &'static str,
    categories: &'static [i64],
    movie_params: &'static [&'static str],
    tv_params: &'static [&'static str],
}

/// The public trackers Melete can add by itself.
pub const PRESETS: [Preset; 4] = [
    Preset {
        key: "thepiratebay",
        name: "The Pirate Bay",
        kind: "apibay",
        url: "https://apibay.org",
        categories: &[2000, 2040, 2045, 3000, 3040, 5000, 5040, 5045],
        // It finds a film by its IMDb id; shows and albums only by name.
        movie_params: &["q", "imdbid"],
        tv_params: &["q"],
    },
    Preset {
        key: "nyaa",
        name: "Nyaa",
        kind: "nyaa",
        url: "https://nyaa.si",
        categories: &[2000, 5070],
        movie_params: &[],
        tv_params: &["q"],
    },
    Preset {
        key: "animetosho",
        name: "AnimeTosho",
        kind: "torznab",
        // Its Torznab feed, which needs no key (feed.animetosho.org no longer exists).
        url: "https://animetosho.org/feed/api",
        categories: &[2020, 5070],
        // Its feed takes plain searches only.
        movie_params: &[],
        tv_params: &["q"],
    },
    Preset {
        key: "uindex",
        name: "Uindex",
        kind: "uindex",
        url: "https://uindex.org",
        categories: &[2000, 5000, 5070],
        // Plain text searches ("Show S01E05") through FlareSolverr.
        movie_params: &[],
        tv_params: &[],
    },
];

/// Why a preset wasn't added.
#[derive(Debug, PartialEq, Eq)]
pub enum NotAdded {
    /// No preset has that key.
    Unknown,
    /// It's behind Cloudflare's check and no FlareSolverr is set.
    NeedsFlareSolverr,
    /// It's there already: its id.
    Already(i64),
}

/// Adds one of [`PRESETS`], turned on; returns its id.
pub async fn add_preset(pool: &SqlitePool, key: &str) -> anyhow::Result<Result<i64, NotAdded>> {
    let Some(preset) = PRESETS.iter().find(|p| p.key == key) else {
        return Ok(Err(NotAdded::Unknown));
    };
    if preset.kind == "uindex" && !torznab::has_flaresolverr() {
        return Ok(Err(NotAdded::NeedsFlareSolverr));
    }
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM indexers WHERE source = 'melete' AND url = ?")
            .bind(preset.url)
            .fetch_optional(pool)
            .await?;
    if let Some(id) = existing {
        return Ok(Err(NotAdded::Already(id)));
    }
    let id = sqlx::query_scalar(
        "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
             tv_search_params, source, kind, added_at)
         VALUES (?, ?, '', 1, ?, ?, ?, 'melete', ?, ?) RETURNING id",
    )
    .bind(preset.name)
    .bind(preset.url)
    .bind(serde_json::to_string(preset.categories)?)
    .bind(serde_json::to_string(preset.movie_params)?)
    .bind(serde_json::to_string(preset.tv_params)?)
    .bind(preset.kind)
    .bind(unix_now())
    .fetch_one(pool)
    .await?;
    Ok(Ok(id))
}

/// Why an indexer added by hand wasn't added.
#[derive(Debug, PartialEq, Eq)]
pub enum NotAddedOwn {
    /// The name or address isn't usable; why.
    Invalid(&'static str),
    /// An indexer at that address is there already: its id.
    Already(i64),
    /// Asking it what it can do failed; why (never with its key or address).
    Unanswered(String),
}

/// Adds a Torznab indexer by hand: any Torznab address with its API key, such as a
/// Jackett instance or a tracker's own feed. It's asked what it can do first (`t=caps`), and its
/// categories and search parameters come from that answer; one that doesn't answer isn't added.
/// The key is kept like every indexer's: searches need it, and it's never shown back.
pub async fn add_torznab(
    pool: &SqlitePool,
    client: &torznab::Client,
    name: &str,
    url: &str,
    api_key: &str,
) -> anyhow::Result<Result<i64, NotAddedOwn>> {
    let (name, url, api_key) = (name.trim(), url.trim(), api_key.trim());
    if name.is_empty() || name.chars().count() > 100 {
        return Ok(Err(NotAddedOwn::Invalid("the name is 1 to 100 characters")));
    }
    if api_key.chars().count() > 200 {
        return Ok(Err(NotAddedOwn::Invalid(
            "the API key is at most 200 characters",
        )));
    }
    let parsed = match reqwest::Url::parse(url) {
        Ok(parsed) if url.len() <= 500 => parsed,
        _ => return Ok(Err(NotAddedOwn::Invalid("the address isn't a web address"))),
    };
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Ok(Err(NotAddedOwn::Invalid(
            "the address starts with http:// or https://",
        )));
    }
    // A key goes in its own field, never in the address, where it could end up shown or logged.
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Ok(Err(NotAddedOwn::Invalid(
            "the address has no user, password, ?query or #part; put the key in its own field",
        )));
    }
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM indexers WHERE url = ?")
        .bind(url)
        .fetch_optional(pool)
        .await?;
    if let Some(id) = existing {
        return Ok(Err(NotAddedOwn::Already(id)));
    }
    let endpoint = torznab::Endpoint { url, api_key };
    let caps = match client.caps(&endpoint).await {
        Ok(caps) => caps,
        Err(err) => return Ok(Err(NotAddedOwn::Unanswered(err.to_string()))),
    };
    let id = sqlx::query_scalar(
        "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
             tv_search_params, source, kind, added_at)
         VALUES (?, ?, ?, 1, ?, ?, ?, 'melete', 'torznab', ?) RETURNING id",
    )
    .bind(name)
    .bind(url)
    .bind(api_key)
    .bind(serde_json::to_string(&caps.categories)?)
    .bind(serde_json::to_string(&caps.movie_search)?)
    .bind(serde_json::to_string(&caps.tv_search)?)
    .bind(unix_now())
    .fetch_one(pool)
    .await?;
    Ok(Ok(id))
}

#[cfg(test)]
mod own_tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::{RawQuery, State};

    use super::*;
    use crate::db;

    /// A Torznab feed that answers `caps` (or an error), and logs what it was asked.
    async fn feed(answer: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let router = axum::Router::new()
            .route(
                "/api",
                axum::routing::get(
                    move |RawQuery(q): RawQuery, State(asked): State<Arc<Mutex<Vec<String>>>>| async move {
                        asked.lock().unwrap().push(q.unwrap_or_default());
                        answer
                    },
                ),
            )
            .with_state(asked.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, asked)
    }

    const CAPS: &str = r#"<?xml version="1.0"?><caps>
        <searching><search available="yes" supportedParams="q"/>
          <tv-search available="yes" supportedParams="q,season,ep,tvdbid"/>
          <movie-search available="yes" supportedParams="q,imdbid"/></searching>
        <categories><category id="2000" name="Movies"><subcat id="2040" name="HD"/></category>
          <category id="5000" name="TV"/></categories></caps>"#;

    #[tokio::test]
    async fn adds_a_torznab_feed_from_what_it_says_it_can_do() {
        let pool = db::open_in_memory().await;
        let client = torznab::Client::new().unwrap();
        let (url, asked) = feed(CAPS).await;
        let id = add_torznab(&pool, &client, " My Jackett ", &url, "the-key")
            .await
            .unwrap()
            .unwrap();
        assert!(asked.lock().unwrap()[0].contains("t=caps"));
        let added = list(&pool).await.unwrap();
        let mine = added.iter().find(|i| i.id == id).unwrap();
        assert_eq!(
            (
                mine.name.as_str(),
                mine.kind.as_str(),
                mine.source.as_str(),
                mine.enabled
            ),
            ("My Jackett", "torznab", "melete", true)
        );
        assert_eq!(mine.categories, [2000, 2040, 5000]);
        assert_eq!(mine.tv_search_params, ["q", "season", "ep", "tvdbid"]);
        assert_eq!(mine.movie_search_params, ["q", "imdbid"]);
        let key: String = sqlx::query_scalar("SELECT api_key FROM indexers WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(key, "the-key", "kept for searches");
        assert_eq!(
            add_torznab(&pool, &client, "Again", &url, "k")
                .await
                .unwrap(),
            Err(NotAddedOwn::Already(id))
        );
    }

    #[tokio::test]
    async fn refuses_unusable_addresses_and_feeds_that_dont_answer() {
        let pool = db::open_in_memory().await;
        let client = torznab::Client::new().unwrap();
        for (name, url) in [
            ("", "http://192.168.1.10:9117/api"),
            ("x", "not an address"),
            ("x", "ftp://192.168.1.10/api"),
            ("x", "http://alice:secret@192.168.1.10/api"),
            ("x", "http://192.168.1.10/api?apikey=secret"),
            ("x", "http://192.168.1.10/api#k"),
        ] {
            assert!(
                matches!(
                    add_torznab(&pool, &client, name, url, "k").await.unwrap(),
                    Err(NotAddedOwn::Invalid(_))
                ),
                "{name} {url}"
            );
        }
        let (url, _) =
            feed(r#"<?xml version="1.0"?><error code="100" description="Invalid API Key"/>"#).await;
        let refused = add_torznab(&pool, &client, "Wrong key", &url, "the-secret-key")
            .await
            .unwrap();
        let Err(NotAddedOwn::Unanswered(why)) = refused else {
            panic!("{refused:?}");
        };
        assert!(
            !why.contains("the-secret-key") && !why.contains(&url),
            "{why}"
        );
        assert!(list(&pool).await.unwrap().is_empty(), "nothing added");
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn presets_are_added_once_and_left_by_prowlarr_imports() {
        let pool = db::open_in_memory().await;
        let tpb = add_preset(&pool, "thepiratebay").await.unwrap().unwrap();
        assert_eq!(
            add_preset(&pool, "thepiratebay").await.unwrap(),
            Err(NotAdded::Already(tpb))
        );
        assert_eq!(
            add_preset(&pool, "worldtorrent").await.unwrap(),
            Err(NotAdded::Unknown)
        );
        if !torznab::has_flaresolverr() {
            assert_eq!(
                add_preset(&pool, "uindex").await.unwrap(),
                Err(NotAdded::NeedsFlareSolverr)
            );
        }
        add_preset(&pool, "nyaa").await.unwrap().unwrap();
        add_preset(&pool, "animetosho").await.unwrap().unwrap();
        let listed = list(&pool).await.unwrap();
        let kinds: Vec<(&str, &str, &str)> = listed
            .iter()
            .map(|i| (i.name.as_str(), i.kind.as_str(), i.source.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("AnimeTosho", "torznab", "melete"),
                ("Nyaa", "nyaa", "melete"),
                ("The Pirate Bay", "apibay", "melete")
            ]
        );
        // A Prowlarr import replaces Prowlarr's indexers only.
        sqlx::query("DELETE FROM indexers WHERE source = 'prowlarr'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(list(&pool).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn turned_off_indexers_are_not_searched_and_removed_ones_leave_results() {
        let pool = db::open_in_memory().await;
        let tpb = add_preset(&pool, "thepiratebay").await.unwrap().unwrap();
        let nyaa = add_preset(&pool, "nyaa").await.unwrap().unwrap();
        let searched = || async {
            crate::search::enabled_indexers(&pool)
                .await
                .unwrap()
                .iter()
                .map(|i| i.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(searched().await, [nyaa, tpb]);
        update(&pool, tpb, "The Pirate Bay", false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(searched().await, [nyaa]);
        assert_eq!(
            update(&pool, 999, "x", true).await.unwrap(),
            Err(NotChanged::NotFound)
        );

        // A search Nyaa answered: its result stays, no longer linked to Nyaa.
        sqlx::query(
            r#"INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
               VALUES (1, 1, 'Dune', 1, 'manual', 0);
               INSERT INTO searches (id, wanted_movie_id, searched_at, indexers)
               VALUES (1, 1, 0, '[{"indexer_id": 2, "name": "Nyaa", "results": 1}]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO release_decisions (search_id, indexer_id, title, accepted, decision)
             VALUES (1, ?, 'Dune.2021.1080p', 1, '{}')",
        )
        .bind(nyaa)
        .execute(&pool)
        .await
        .unwrap();
        assert!(remove(&pool, nyaa).await.unwrap());
        assert!(!remove(&pool, nyaa).await.unwrap());
        let left: (Option<i64>, String) = sqlx::query_as(
            "SELECT d.indexer_id, s.indexers
             FROM release_decisions d JOIN searches s ON s.id = d.search_id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(left.0, None);
        assert!(left.1.contains("Nyaa"));
        assert!(searched().await.is_empty());
    }
}
