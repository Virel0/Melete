//! Titles an admin took off the wanted list, and letting one come back. Imports and
//! import lists leave a removed title off; here an admin sees them with
//! when and by whom, forgets a removal so the next import or sync may add the title again, or
//! adds it again now, as an approved request would.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::music_requests::{self, Asked};
use crate::requests::{self, Choice, Refused};
use crate::wanted_edit::Kind;
use crate::{musicbrainz, tmdb};

/// A title taken off the wanted list.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Removal {
    /// movie, series, artist or album.
    pub kind: String,
    /// What imports know it by: a movie's TMDB id, a series' TVDB id, an artist's or album's
    /// MusicBrainz id.
    pub key: String,
    /// Its title (an artist's name) when it was taken off.
    pub title: String,
    /// When it was taken off, in Unix seconds.
    pub removed_at: i64,
    /// The admin who took it off, by username; unknown for older removals.
    pub removed_by: Option<String>,
}

/// Every removal, newest first.
pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<Removal>> {
    sqlx::query_as(
        "SELECT r.kind, r.key, r.title, r.removed_at, u.username AS removed_by
         FROM wanted_removed r LEFT JOIN users u ON u.id = r.removed_by
         ORDER BY r.removed_at DESC, r.title",
    )
    .fetch_all(pool)
    .await
}

/// Forgets a removal, so the next import or list sync may add the title again; false when
/// there was none.
pub async fn allow(pool: &SqlitePool, kind: Kind, key: &str) -> sqlx::Result<bool> {
    Ok(
        sqlx::query("DELETE FROM wanted_removed WHERE kind = ? AND key = ?")
            .bind(kind_name(kind))
            .bind(key)
            .execute(pool)
            .await?
            .rows_affected()
            > 0,
    )
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Movie => "movie",
        Kind::Series => "series",
        Kind::Artist => "artist",
        Kind::Album => "album",
    }
}

/// Why a title wasn't added again.
#[derive(Debug, PartialEq)]
pub enum NotAdded {
    /// No such removal.
    NotFound,
    Refused(Refused),
}

/// Adds a removed title to the wanted list again now, as an approved request would put it
/// there (where approved ones go; a series with all its seasons, a whole artist with every
/// album and their new ones), and forgets the removal. Its wanted id.
pub async fn add_again(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    mb: Option<&musicbrainz::Client>,
    kind: Kind,
    key: &str,
) -> anyhow::Result<Result<i64, NotAdded>> {
    let title: Option<String> =
        sqlx::query_scalar("SELECT title FROM wanted_removed WHERE kind = ? AND key = ?")
            .bind(kind_name(kind))
            .bind(key)
            .fetch_optional(pool)
            .await?;
    let Some(title) = title else {
        return Ok(Err(NotAdded::NotFound));
    };
    let choice = Choice::default();
    let wanted = match kind {
        Kind::Movie => match key.parse::<i64>() {
            Ok(tmdb_id) => {
                requests::want_from_tmdb(pool, tmdb, "movie", tmdb_id, None, &choice).await?
            }
            Err(_) => Err(Refused::Invalid(format!("{key:?} isn't a TMDB id"))),
        },
        Kind::Series => match series_tmdb_id(pool, tmdb, key).await? {
            Ok(tmdb_id) => {
                requests::want_from_tmdb(pool, tmdb, "series", tmdb_id, None, &choice).await?
            }
            Err(refused) => Err(refused),
        },
        Kind::Artist | Kind::Album => {
            let album = kind == Kind::Album;
            let asked = Asked {
                kind: if album { "album" } else { "artist" }.into(),
                title,
                artist: None,
                mb_artist_id: (!album).then(|| key.to_owned()),
                mb_release_group_id: album.then(|| key.to_owned()),
                mb_recording_id: None,
            };
            music_requests::approve(pool, mb, &asked, &choice)
                .await?
                .map(|(_, id)| id)
        }
    };
    match wanted {
        Ok(id) => {
            allow(pool, kind, key).await?;
            Ok(Ok(id))
        }
        Err(refused) => Ok(Err(NotAdded::Refused(refused))),
    }
}

/// A series' TMDB id from its TVDB id: as TMDB told Melete before, else asked.
async fn series_tmdb_id(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    tvdb: &str,
) -> anyhow::Result<Result<i64, Refused>> {
    let Ok(tvdb) = tvdb.parse::<i64>() else {
        return Ok(Err(Refused::Invalid(format!("{tvdb:?} isn't a TVDB id"))));
    };
    let known: Option<i64> =
        sqlx::query_scalar("SELECT tmdb_id FROM tmdb_series WHERE tvdb_id = ? ORDER BY tmdb_id")
            .bind(tvdb)
            .fetch_optional(pool)
            .await?;
    if let Some(id) = known {
        return Ok(Ok(id));
    }
    let Some(tmdb) = tmdb else {
        return Ok(Err(Refused::NoMetadata));
    };
    let found = match tmdb.find(tmdb::ExternalId::Tvdb(tvdb)).await {
        Ok(found) => found,
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "TMDB didn't find a series by TVDB id"
            );
            return Ok(Err(Refused::Outside("TMDB")));
        }
    };
    Ok(found
        .tv_results
        .first()
        .map(|s| s.id)
        .ok_or_else(|| Refused::Unavailable("TMDB knows no series by its TVDB id".into())))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db;
    use crate::tmdb::testing::{FakeTmdb, Reply};
    use crate::wanted_edit;

    async fn admin(pool: &SqlitePool) -> i64 {
        crate::auth::create_user(pool, "alice", "alice-password".into(), true)
            .await
            .unwrap()
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn lists_removals_with_who_and_forgets_one() {
        let pool = db::open_in_memory().await;
        let alice = admin(&pool).await;
        sqlx::query(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
                 VALUES (1, 438631, 'Dune', 1, 'radarr', 0);
             INSERT INTO wanted_removed (kind, key, title, removed_at)
                 VALUES ('album', 'rg-1', 'Kid A', 5);",
        )
        .execute(&pool)
        .await
        .unwrap();
        wanted_edit::remove_by(&pool, Kind::Movie, 1, Some(alice))
            .await
            .unwrap()
            .unwrap();
        let listed = list(&pool).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            (
                listed[0].kind.as_str(),
                listed[0].key.as_str(),
                listed[0].title.as_str()
            ),
            ("movie", "438631", "Dune")
        );
        assert_eq!(listed[0].removed_by.as_deref(), Some("alice"));
        assert_eq!(listed[1].removed_by, None, "taken off before who was kept");

        // Forgotten: an import may add it again.
        let removed = || async {
            let mut conn = pool.acquire().await.unwrap();
            wanted_edit::removed_here(&mut conn, Kind::Movie, "438631")
                .await
                .unwrap()
        };
        assert!(removed().await);
        assert!(allow(&pool, Kind::Movie, "438631").await.unwrap());
        assert!(!removed().await);
        assert!(!allow(&pool, Kind::Movie, "438631").await.unwrap());
        assert_eq!(list(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn adds_a_movie_or_series_again_where_approved_ones_go() {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'), (2, 'shows', 'shows', '/s');
             INSERT INTO wanted_removed (kind, key, title, removed_at) VALUES
                 ('movie', '438631', 'Dune', 1), ('series', '424536', 'Frieren', 2),
                 ('series', '1', 'Unknown', 3)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let fake = FakeTmdb::default();
        fake.reply(
            "/movie/438631",
            vec![Reply::ok(
                json!({ "id": 438631, "title": "Dune", "release_date": "2021-09-15",
                "original_language": "en", "genres": [] }),
            )],
        )
        .reply(
            "/find/424536?external_source=tvdb_id",
            vec![Reply::ok(
                json!({ "movie_results": [], "tv_results": [{ "id": 209867, "name": "Frieren" }] }),
            )],
        )
        .reply(
            "/find/1?external_source=tvdb_id",
            vec![Reply::ok(json!({ "movie_results": [], "tv_results": [] }))],
        )
        .reply(
            "/tv/209867?append_to_response=external_ids",
            vec![Reply::ok(
                json!({ "id": 209867, "name": "Frieren", "seasons": [{ "season_number": 1 }],
                "external_ids": { "tvdb_id": 424536 } }),
            )],
        );
        let tmdb = fake.start().await;

        let movie = add_again(&pool, Some(&tmdb), None, Kind::Movie, "438631")
            .await
            .unwrap()
            .unwrap();
        let (tmdb_id, monitored, library): (i64, bool, i64) =
            sqlx::query_as("SELECT tmdb_id, monitored, library_id FROM wanted_movies WHERE id = ?")
                .bind(movie)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((tmdb_id, monitored, library), (438631, true, 1));
        add_again(&pool, Some(&tmdb), None, Kind::Series, "424536")
            .await
            .unwrap()
            .unwrap();
        let seasons: Vec<(i64, bool)> =
            sqlx::query_as("SELECT season, monitored FROM wanted_seasons ORDER BY season")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(seasons, [(1, true)]);
        // Forgotten once back; one TMDB can't find stays as it was.
        let left: Vec<String> = list(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(left, ["Unknown"]);
        assert!(matches!(
            add_again(&pool, Some(&tmdb), None, Kind::Series, "1")
                .await
                .unwrap(),
            Err(NotAdded::Refused(Refused::Unavailable(_)))
        ));
        assert_eq!(list(&pool).await.unwrap().len(), 1);
        assert_eq!(
            add_again(&pool, Some(&tmdb), None, Kind::Movie, "438631")
                .await
                .unwrap(),
            Err(NotAdded::NotFound)
        );
        // Without TMDB, a movie can't be looked up.
        sqlx::query(
            "INSERT INTO wanted_removed (kind, key, title, removed_at) VALUES ('movie', '7', 'Seven', 4)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            add_again(&pool, None, None, Kind::Movie, "7")
                .await
                .unwrap(),
            Err(NotAdded::Refused(Refused::NoMetadata))
        );
    }

    #[tokio::test]
    async fn adds_an_album_again_with_its_artist() {
        use crate::musicbrainz::testing::{OK_COMPUTER, RADIOHEAD};
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', '/a');
             INSERT INTO wanted_removed (kind, key, title, removed_at) VALUES ('album', ?, 'OK Computer', 1)",
        )
        .bind(OK_COMPUTER)
        .execute(&pool)
        .await
        .unwrap();
        // Without MusicBrainz it can't be looked up.
        assert_eq!(
            add_again(&pool, None, None, Kind::Album, OK_COMPUTER)
                .await
                .unwrap(),
            Err(NotAdded::Refused(Refused::NoMusicBrainz))
        );
        let (mb, _) = crate::musicbrainz::testing::start(0).await;
        let album = add_again(&pool, None, Some(&mb), Kind::Album, OK_COMPUTER)
            .await
            .unwrap()
            .unwrap();
        let (title, monitored, artist): (String, bool, String) = sqlx::query_as(
            "SELECT w.title, w.monitored, a.mb_artist_id FROM wanted_albums w
             JOIN wanted_artists a ON a.id = w.wanted_artist_id WHERE w.id = ?",
        )
        .bind(album)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (title.as_str(), monitored, artist.as_str()),
            ("OK Computer", true, RADIOHEAD)
        );
        assert!(list(&pool).await.unwrap().is_empty());
    }
}
