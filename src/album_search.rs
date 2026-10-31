//! Searching indexers for a wanted album: every turned-on indexer's music
//! categories at once, each result checked against the album (artist, then the album's title,
//! then nothing but a year, a tag or an edition word) and decided on with the artist's music
//! profile. Every result is kept with its reasons, as video's are, so why a release was grabbed
//! or passed over can be shown. With slskd set up, Soulseek is asked at the same time, and its
//! users' album folders
//! are kept beside the torrents, so the best accepted result wins whichever source it's from.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::music_profiles::{self, MusicProfile};
use crate::music_release::{self, MusicRelease};
use crate::search::{IndexerOutcome, IndexerRow, Refused, comparable, enabled_indexers};
use crate::{slskd, soulseek, torznab};

/// Searches are kept this long, as video's are.
const KEEP_SECONDS: i64 = 30 * 24 * 3600;

/// Words that may follow an album's title in a release name without making it another album:
/// tags, formats and editions.
const AFTER_TITLE: &[&str] = &[
    "flac",
    "webflac",
    "alac",
    "mp3",
    "aac",
    "m4a",
    "ogg",
    "opus",
    "wav",
    "ape",
    "wv",
    "wavpack",
    "web",
    "webdl",
    "cd",
    "cdrip",
    "cdda",
    "vinyl",
    "vinylrip",
    "24bit",
    "16bit",
    "24",
    "16",
    "320",
    "256",
    "v0",
    "v2",
    "kbps",
    "hi",
    "res",
    "hires",
    "deluxe",
    "edition",
    "expanded",
    "remaster",
    "remastered",
    "anniversary",
    "bonus",
    "special",
    "limited",
    "reissue",
    "ep",
    "single",
    "lp",
    "mono",
    "stereo",
    "explicit",
];

/// A wanted album, as searched for.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WantedAlbum {
    /// The wanted album's row id.
    pub id: i64,
    /// The album's title.
    pub title: String,
    /// Its artist's name.
    pub artist: String,
    /// The wanted artist's row id.
    pub wanted_artist_id: i64,
}

const WANTED_ALBUM: &str = "SELECT w.id, w.title, a.name AS artist, a.id AS wanted_artist_id
     FROM wanted_albums w JOIN wanted_artists a ON a.id = w.wanted_artist_id";

/// The albums RSS looks out for: monitored, by a monitored artist.
pub async fn monitored(pool: &SqlitePool) -> sqlx::Result<Vec<WantedAlbum>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{WANTED_ALBUM} WHERE w.monitored = 1 AND a.monitored = 1 ORDER BY w.id"
    )))
    .fetch_all(pool)
    .await
}

/// Whether a wanted album needs nothing: it's on disk (its release group, by tags or by
/// matching), or a download of it hasn't failed.
pub async fn taken(pool: &SqlitePool, album_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM wanted_albums w JOIN music_albums m
                        ON m.missing_since IS NULL
                       AND w.mb_release_group_id IN (m.mb_release_group_id, m.match_release_group_id)
                        WHERE w.id = ?1)
             OR EXISTS (SELECT 1 FROM downloads WHERE wanted_album_id = ?1 AND state <> 'failed')",
    )
    .bind(album_id)
    .fetch_one(pool)
    .await
}

/// Where `phrase` sits in `words` as whole words, from `from` on: the index after it.
fn find(words: &[&str], phrase: &[&str], from: usize) -> Option<usize> {
    // A phrase longer than the words can't be in them (and would slice past their end).
    if phrase.is_empty() || words.len() < phrase.len() {
        return None;
    }
    (from..=words.len() - phrase.len())
        .find(|&i| words[i..i + phrase.len()] == *phrase)
        .map(|i| i + phrase.len())
}

/// Why a release isn't the album, or None when it is: the artist, then the album's title,
/// then nothing but a year, a tag or an edition word — so "Live at Wembley" isn't the album
/// "Live", and a discography isn't any one album.
pub fn mismatch(name: &str, artist: &str, album: &str) -> Option<String> {
    mismatch_as(name, artist, album, "album")
}

/// [`mismatch`] for a title of any kind, `what` naming it in the reasons ("album", "song").
pub fn mismatch_as(name: &str, artist: &str, album: &str, what: &str) -> Option<String> {
    let name = comparable(name);
    let words: Vec<&str> = name.split(' ').collect();
    let artist_c = comparable(artist);
    let artist_words: Vec<&str> = artist_c.split(' ').collect();
    // A compilation's releases name it "VA", or name no artist at all: its title must then
    // be there in full on its own.
    let compilation = artist_c == "various artists";
    let after_artist = find(&words, &artist_words, 0)
        .or_else(|| compilation.then(|| find(&words, &["va"], 0)).flatten())
        .or(compilation.then_some(0));
    let Some(after_artist) = after_artist else {
        return Some(format!("it doesn't name the artist ({artist})"));
    };
    let album_c = comparable(album);
    let album_words: Vec<&str> = album_c.split(' ').collect();
    let Some(after_album) = find(&words, &album_words, after_artist) else {
        return Some(format!("it doesn't name the {what} ({album})"));
    };
    match words.get(after_album) {
        None => None,
        Some(next) if AFTER_TITLE.contains(next) => None,
        Some(next)
            if next.len() == 4
                && (next.starts_with("19") || next.starts_with("20"))
                && next.bytes().all(|b| b.is_ascii_digit()) =>
        {
            None
        }
        Some(next) => Some(format!(
            "it names another {what}: \"{album}\" goes on with \"{next}\""
        )),
    }
}

/// A result's decision, as stored with it.
#[derive(Debug, Serialize)]
pub struct AlbumDecision {
    /// Whether the result may be grabbed: its name is the album's and the profile allows its
    /// quality.
    pub accepted: bool,
    /// Its quality, one of [`music_release::QUALITIES`].
    pub quality: &'static str,
    /// The quality's rank in the profile; higher is better.
    pub rank: Option<i64>,
    /// The year its name gives, when it gives one.
    pub year: Option<i64>,
    /// Where it was ripped from, when its name says.
    pub source: Option<music_release::Source>,
    /// Why it was refused; empty when accepted.
    pub reasons: Vec<String>,
}

/// Decides a result by its name: whether it's `album_title` by `artist`, and whether `profile`
/// allows its quality.
pub fn decide(
    name: &str,
    album_title: &str,
    artist: &str,
    profile: &MusicProfile,
) -> AlbumDecision {
    let release: MusicRelease = music_release::parse(name);
    let quality = music_profiles::decide(profile, &release);
    let mut reasons: Vec<String> = mismatch(name, artist, album_title).into_iter().collect();
    reasons.extend(quality.reasons);
    AlbumDecision {
        accepted: reasons.is_empty(),
        quality: release.quality,
        rank: quality.rank,
        year: release.year,
        source: release.source,
        reasons,
    }
}

/// The query for one indexer: a plain search in its music categories, which every Torznab
/// indexer takes.
fn params(
    indexer: &IndexerRow,
    album: &WantedAlbum,
) -> Result<Vec<(&'static str, String)>, String> {
    let categories: Vec<String> = indexer
        .categories
        .iter()
        .filter(|c| (3000..4000).contains(*c))
        .map(i64::to_string)
        .collect();
    if categories.is_empty() {
        return Err("it has no music categories".into());
    }
    Ok(vec![
        ("cat", categories.join(",")),
        ("t", "search".into()),
        ("q", format!("{} {}", album.artist, album.title)),
    ])
}

/// Searches every turned-on indexer, and Soulseek when `slskd` is given, for a wanted album and
/// stores what they found, decided on; returns the search's id.
pub async fn search(
    pool: &SqlitePool,
    client: &torznab::Client,
    slskd: Option<&slskd::Client>,
    wanted_album_id: i64,
) -> anyhow::Result<Result<i64, Refused>> {
    search_skipping(pool, client, slskd, wanted_album_id, &[]).await
}

/// Like [`search`], leaving out the indexers in `skip`: ones a search-all-missing run found at
/// their limit. Soulseek is still asked.
pub async fn search_skipping(
    pool: &SqlitePool,
    client: &torznab::Client,
    slskd: Option<&slskd::Client>,
    wanted_album_id: i64,
    skip: &[i64],
) -> anyhow::Result<Result<i64, Refused>> {
    let album: Option<WantedAlbum> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{WANTED_ALBUM} WHERE w.id = ?"
    )))
    .bind(wanted_album_id)
    .fetch_optional(pool)
    .await?;
    let Some(album) = album else {
        return Ok(Err(Refused::NotWanted));
    };
    let Some(profile) = music_profiles::for_artist(pool, album.wanted_artist_id).await? else {
        return Ok(Err(Refused::NoProfile(
            "the artist has no music profile; import Lidarr's profiles or choose one".into(),
        )));
    };
    let indexers: Vec<IndexerRow> = enabled_indexers(pool)
        .await?
        .into_iter()
        .filter(|i| !skip.contains(&i.id))
        .collect();
    if indexers.is_empty() && slskd.is_none() {
        return Ok(Err(Refused::NoIndexers));
    }
    let asking_soulseek = async {
        let client = slskd?;
        Some(soulseek::search_loosely(client, &album.artist, &album.title).await)
    };
    let (outcomes, answered) =
        tokio::join!(ask_indexers(client, &indexers, &album), asking_soulseek);
    let outcomes = outcomes?;
    let soulseek = answered.map(|answer| soulseek::found(answer, &album, &profile));

    let summary: Vec<&IndexerOutcome> = outcomes.iter().flatten().map(|(o, _)| o).collect();
    let decided: Vec<(i64, &torznab::Item, AlbumDecision)> = outcomes
        .iter()
        .flatten()
        .flat_map(|(outcome, items)| {
            items.iter().map(|item| {
                let decision = decide(&item.title, &album.title, &album.artist, &profile);
                (outcome.indexer_id, item, decision)
            })
        })
        .collect();
    Ok(Ok(store(
        pool,
        album.id,
        profile.id,
        None,
        &summary,
        &decided,
        soulseek.as_ref(),
    )
    .await?))
}

/// Every indexer's answer, in the indexers' order.
async fn ask_indexers(
    client: &torznab::Client,
    indexers: &[IndexerRow],
    album: &WantedAlbum,
) -> anyhow::Result<Vec<Option<(IndexerOutcome, Vec<torznab::Item>)>>> {
    let mut tasks = tokio::task::JoinSet::new();
    for (position, indexer) in indexers.iter().enumerate() {
        let params = params(indexer, album);
        let client = client.clone();
        let (url, api_key, kind) = (indexer.url.clone(), indexer.api_key.clone(), indexer.kind);
        tasks.spawn(async move {
            let result = match params {
                Ok(params) => {
                    let pairs: Vec<(&str, &str)> =
                        params.iter().map(|(k, v)| (*k, v.as_str())).collect();
                    let endpoint = torznab::Endpoint {
                        url: &url,
                        api_key: &api_key,
                    };
                    client
                        .search_as(kind, &endpoint, &pairs)
                        .await
                        .map_err(|err| err.to_string())
                }
                Err(why) => Err(why),
            };
            (position, result)
        });
    }
    let mut outcomes: Vec<Option<(IndexerOutcome, Vec<torznab::Item>)>> =
        indexers.iter().map(|_| None).collect();
    while let Some(joined) = tasks.join_next().await {
        let (position, result) = joined?;
        let indexer = &indexers[position];
        let (error, items) = match result {
            Ok(items) => (None, items),
            Err(error) => (Some(error), Vec::new()),
        };
        let outcome = IndexerOutcome {
            indexer_id: indexer.id,
            name: indexer.name.clone(),
            results: items.len(),
            error,
        };
        outcomes[position] = Some((outcome, items));
    }
    Ok(outcomes)
}

/// Keeps a search for an album, with every result and its decision; returns its id. An RSS
/// sync's are linked to it. `soulseek` is what asking slskd gave, when it was asked.
pub async fn store(
    pool: &SqlitePool,
    album_id: i64,
    profile_id: i64,
    rss_sync_id: Option<i64>,
    summary: &[&IndexerOutcome],
    decided: &[(i64, &torznab::Item, AlbumDecision)],
    soulseek: Option<&soulseek::Found>,
) -> anyhow::Result<i64> {
    let now = unix_now();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM searches WHERE searched_at < ?")
        .bind(now - KEEP_SECONDS)
        .execute(&mut *tx)
        .await?;
    let search_id: i64 = sqlx::query_scalar(
        "INSERT INTO searches (wanted_album_id, music_profile_id, searched_at, indexers, rss_sync_id,
             soulseek)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(album_id)
    .bind(profile_id)
    .bind(now)
    .bind(serde_json::to_string(&summary)?)
    .bind(rss_sync_id)
    .bind(soulseek.map(|f| serde_json::to_string(&f.outcome)).transpose()?)
    .fetch_one(&mut *tx)
    .await?;
    let sealing = crate::secrets::sealing(&mut *tx).await?;
    let link =
        |what, link: &Option<String>| crate::secrets::link_to_store(sealing, what, link.as_deref());
    for (indexer_id, item, decision) in decided {
        sqlx::query(
            "INSERT INTO release_decisions (search_id, indexer_id, title, size, seeders, peers,
                 published, infohash, download_url, guid, accepted, decision)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(search_id)
        .bind(indexer_id)
        .bind(&item.title)
        .bind(item.size.and_then(|s| i64::try_from(s).ok()))
        .bind(item.seeders)
        .bind(item.peers)
        .bind(item.published)
        .bind(&item.infohash)
        .bind(link(crate::secrets::DOWNLOAD_URL, &item.download_url)?)
        .bind(link(crate::secrets::GUID, &item.guid)?)
        .bind(decision.accepted)
        .bind(serde_json::to_string(&decision)?)
        .execute(&mut *tx)
        .await?;
    }
    for (candidate, name, decision) in soulseek.map(|f| f.results.as_slice()).unwrap_or_default() {
        sqlx::query(
            "INSERT INTO release_decisions (search_id, title, size, accepted, decision, soulseek)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(search_id)
        .bind(name)
        .bind(candidate.size())
        .bind(decision.accepted)
        .bind(serde_json::to_string(&decision)?)
        .bind(serde_json::to_string(&candidate.source())?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(search_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compilation_may_name_no_artist_but_its_whole_title() {
        let title = "Cyberpunk: Edgerunners 2 (Original Series Soundtrack)";
        let named = "Cyberpunk Edgerunners 2 (Original Series Soundtrack) [2026] [FLAC]";
        assert_eq!(mismatch(named, "Various Artists", title), None);
        assert_eq!(
            mismatch(&format!("VA - {named}"), "Various Artists", title),
            None
        );
        assert!(mismatch("Cyberpunk Edgerunners 2 FLAC", "Various Artists", title).is_some());
        // Only a compilation goes without its artist.
        assert!(mismatch(named, "Some Composer", title).is_some());
    }

    #[test]
    fn a_name_shorter_than_the_artist_or_album_is_neither() {
        // Used to slice past the name's end and panic.
        assert_eq!(
            mismatch("Muse", "The National", "Boxer").as_deref(),
            Some("it doesn't name the artist (The National)")
        );
        assert_eq!(
            mismatch("Radiohead", "Radiohead", "OK Computer").as_deref(),
            Some("it doesn't name the album (OK Computer)")
        );
    }

    #[test]
    fn a_release_is_the_album_only_when_it_says_so() {
        for (name, artist, album, ok) in [
            (
                "Radiohead - OK Computer (1997) [FLAC]",
                "Radiohead",
                "OK Computer",
                true,
            ),
            (
                "Radiohead-OK_Computer-WEBFLAC-2016-GROUP",
                "Radiohead",
                "OK Computer",
                true,
            ),
            (
                "Radiohead - OK Computer [Deluxe Edition] (2017) FLAC",
                "Radiohead",
                "OK Computer",
                true,
            ),
            ("Radiohead - OK Computer", "Radiohead", "OK Computer", true),
            (
                "The National - Boxer (2007) [MP3 V0]",
                "The National",
                "Boxer",
                true,
            ),
            ("VA - Now 1 (1983) [FLAC]", "Various Artists", "Now 1", true),
            (
                "Beyoncé - Renaissance (2022) [FLAC]",
                "Beyonce",
                "Renaissance",
                true,
            ),
            // Another album that starts the same way.
            (
                "Radiohead - OK Computer OKNOTOK 1997 2017 [FLAC]",
                "Radiohead",
                "OK Computer",
                false,
            ),
            (
                "Muse - Live at Wembley (2008) [FLAC]",
                "Muse",
                "Live",
                false,
            ),
            // Not the album at all.
            (
                "Radiohead - Discography (1993-2016) [FLAC]",
                "Radiohead",
                "OK Computer",
                false,
            ),
            (
                "Thom Yorke - OK Computer Covers (2020) [FLAC]",
                "Radiohead",
                "OK Computer",
                false,
            ),
            // The album before its artist: rare, and not trusted.
            (
                "OK Computer - Radiohead [FLAC]",
                "Radiohead",
                "OK Computer",
                false,
            ),
        ] {
            assert_eq!(
                mismatch(name, artist, album).is_none(),
                ok,
                "{name}: {:?}",
                mismatch(name, artist, album)
            );
        }
    }

    #[test]
    fn decides_with_the_profile_and_the_album_both() {
        let preset = MusicProfile {
            id: 1,
            source: "preset".into(),
            name: "FLAC preferred".into(),
            upgrade_allowed: true,
            cutoff: "Lossless".into(),
            qualities: ["MP3-320", "FLAC"]
                .iter()
                .enumerate()
                .map(|(rank, q)| music_profiles::RankedQuality {
                    quality: (*q).into(),
                    rank: rank as i64,
                    group: None,
                    allowed: true,
                })
                .collect(),
        };
        let good = decide(
            "Radiohead - OK Computer (1997) [FLAC]",
            "OK Computer",
            "Radiohead",
            &preset,
        );
        assert!(
            good.accepted && good.rank == Some(1) && good.year == Some(1997),
            "{good:?}"
        );
        let both = decide("Muse - Live at Wembley [MP3 192]", "Live", "Muse", &preset);
        assert!(!both.accepted);
        assert_eq!(
            both.reasons.len(),
            2,
            "both reasons are given: {:?}",
            both.reasons
        );
    }

    /// An indexer answering every search with `titles`, recording each query it was sent.
    async fn indexer(titles: &[&str]) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use axum::extract::{RawQuery, State};
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let items: String = titles
            .iter()
            .enumerate()
            .map(|(i, title)| {
                format!(
                    r#"<item><title>{title}</title><guid>https://tracker.example/t/{i}</guid>
                    <link>http://indexer.example/download?apikey=secret-key&amp;id={i}</link>
                    <size>500000000</size><torznab:attr name="seeders" value="{}"/></item>"#,
                    10 + i
                )
            })
            .collect();
        let feed = format!(
            r#"<rss xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>{items}</channel></rss>"#
        );
        let router = axum::Router::new()
            .fallback(
                |State((feed, log)): State<(
                    String,
                    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
                )>,
                 RawQuery(query): RawQuery| async move {
                    log.lock().unwrap().push(query.unwrap_or_default());
                    feed
                },
            )
            .with_state((feed, log.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (base, log)
    }

    #[tokio::test]
    async fn searches_music_categories_checks_the_album_and_keeps_every_result() {
        let pool = crate::db::open_in_memory().await;
        let preset = music_profiles::lossless(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at, profile_id)
             VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0, ?)",
        )
        .bind(preset)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(
            "INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg', 'OK Computer', 1, 'lidarr', 0);
             INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, quality_profile, source, added_at)
                 VALUES (2, 'mb-b', 'Nobody', 1, 'all', 'Not Imported', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (8, 2, 'rg2', 'Nothing', 1, 'lidarr', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let client = torznab::Client::new().unwrap();
        assert_eq!(
            search(&pool, &client, None, 7).await.unwrap(),
            Err(Refused::NoIndexers)
        );

        let (base, log) = indexer(&[
            "Radiohead - OK Computer (1997) [MP3 320]",
            "Radiohead - OK Computer (1997) [FLAC]",
            "Radiohead - OK Computer OKNOTOK 1997 2017 [FLAC]",
            "Radiohead - OK Computer (1997) [MP3 192]",
        ])
        .await;
        for (n, name, categories) in [
            (1, "Music", "[3000,3040]"),
            (2, "Video only", "[2000,5000]"),
        ] {
            sqlx::query(
                "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
                     tv_search_params, source, source_id, added_at)
                 VALUES (?, ?, 'secret-key', 1, ?, '[]', '[]', 'prowlarr', ?, 0)",
            )
            .bind(name)
            .bind(format!("{base}/{n}/api"))
            .bind(categories)
            .bind(n)
            .execute(&pool)
            .await
            .unwrap();
        }
        let id = search(&pool, &client, None, 7).await.unwrap().unwrap();
        let view = crate::search::get(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            (view.wanted_album_id, view.music_profile_id, view.profile_id),
            (Some(7), Some(preset), None)
        );
        let queried = log.lock().unwrap().clone();
        assert_eq!(
            queried.len(),
            1,
            "the video-only indexer isn't asked: {queried:?}"
        );
        assert!(
            queried[0].contains("cat=3000%2C3040")
                && queried[0].contains("q=Radiohead+OK+Computer"),
            "{queried:?}"
        );
        assert_eq!(
            view.indexers[1].error.as_deref(),
            Some("it has no music categories")
        );
        // Accepted first, the best quality first among them.
        let summary: Vec<(&str, bool)> = view
            .releases
            .iter()
            .map(|r| (r.title.as_str(), r.accepted))
            .collect();
        assert_eq!(
            summary,
            [
                ("Radiohead - OK Computer (1997) [FLAC]", true),
                ("Radiohead - OK Computer (1997) [MP3 320]", true),
                ("Radiohead - OK Computer OKNOTOK 1997 2017 [FLAC]", false),
                ("Radiohead - OK Computer (1997) [MP3 192]", false),
            ]
        );
        assert_eq!(view.releases[0].decision["quality"], "FLAC");
        let shown = serde_json::to_string(&view).unwrap();
        assert!(
            !shown.contains("secret-key") && !shown.contains("indexer.example"),
            "links stay on the server"
        );

        // No profile, and no such album.
        assert!(matches!(
            search(&pool, &client, None, 8).await.unwrap(),
            Err(Refused::NoProfile(_))
        ));
        assert_eq!(
            search(&pool, &client, None, 99).await.unwrap(),
            Err(Refused::NotWanted)
        );
    }

    #[tokio::test]
    async fn asks_soulseek_beside_the_indexers_and_keeps_its_folders() {
        let pool = crate::db::open_in_memory().await;
        let preset = music_profiles::lossless(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at, profile_id)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0, ?);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg', 'OK Computer', 1, 'lidarr', 0);",
        )
        .bind(preset)
        .execute(&pool)
        .await
        .unwrap();
        let flac = |path: String| serde_json::json!({"filename": path, "size": 25_000_000, "bitDepth": 16});
        let folder = |root: &str, tracks: u32| -> Vec<serde_json::Value> {
            (1..=tracks)
                .map(|n| flac(format!("{root}\\{n:02}.flac")))
                .collect()
        };
        let mut alice = folder("@@music\\Radiohead\\OK Computer (1997)", 12);
        alice.push(serde_json::json!({"filename": "@@music\\Radiohead\\OK Computer (1997)\\cover.jpg", "size": 200_000}));
        let (slskd, seen) = crate::slskd::testing::start(
            serde_json::json!([
                {"username": "alice", "hasFreeUploadSlot": true, "queueLength": 0, "uploadSpeed": 9, "files": alice},
                {"username": "bob", "hasFreeUploadSlot": true, "queueLength": 0, "uploadSpeed": 9,
                 "files": folder("@@b\\Radiohead - OK Computer", 4)},
            ]),
            1,
        )
        .await;
        let client = torznab::Client::new().unwrap();

        // No indexer is needed when slskd is set up.
        let id = search(&pool, &client, Some(&slskd), 7)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seen.lock().unwrap().texts, ["Radiohead OK Computer"]);
        let view = crate::search::get(&pool, id).await.unwrap().unwrap();
        assert!(view.indexers.is_empty());
        assert_eq!(
            view.soulseek,
            Some(soulseek::Outcome {
                results: 2,
                error: None
            })
        );
        // Source, accepted, title, size and indexer.
        type Shown<'a> = (&'a str, bool, &'a str, Option<i64>, Option<i64>);
        let shown: Vec<Shown> = view
            .releases
            .iter()
            .map(|r| (r.source, r.accepted, r.title.as_str(), r.size, r.indexer_id))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "soulseek",
                    true,
                    "Radiohead - OK Computer (1997)",
                    Some(12 * 25_000_000 + 200_000),
                    None
                ),
                (
                    "soulseek",
                    false,
                    "Radiohead - OK Computer",
                    Some(4 * 25_000_000),
                    None
                ),
            ]
        );
        let stored: String =
            sqlx::query_scalar("SELECT soulseek FROM release_decisions WHERE id = ?")
                .bind(view.releases[0].id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let source: soulseek::Source = serde_json::from_str(&stored).unwrap();
        assert_eq!(source.username, "alice");
        assert_eq!(source.folder, "@@music\\Radiohead\\OK Computer (1997)");
        assert_eq!(source.files.len(), 13, "the tracks and the cover");
        assert!(
            !serde_json::to_string(&view).unwrap().contains("alice"),
            "peers' names aren't shown"
        );

        // slskd away: the search still runs, and says why Soulseek gave nothing.
        let away =
            crate::slskd::Client::new("http://127.0.0.1:9", crate::config::Secret::for_tests("k"))
                .unwrap();
        let id = search(&pool, &client, Some(&away), 7)
            .await
            .unwrap()
            .unwrap();
        let view = crate::search::get(&pool, id).await.unwrap().unwrap();
        let outcome = view.soulseek.unwrap();
        assert_eq!(outcome.results, 0);
        assert!(outcome.error.unwrap().starts_with("slskd: "));
        assert!(view.releases.is_empty());
    }
}
