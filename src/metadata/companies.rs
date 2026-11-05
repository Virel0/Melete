//! The companies behind titles whose details were fetched before they were kept: asked for once,
//! one request a title by the TMDB id already used, and only the companies written, so nothing
//! else about the title is fetched again. Titles fetched since carry theirs.

use super::*;

/// Titles filled in one run, so a large library is done over a few runs rather than all at
/// once.
const BATCH: i64 = 200;

/// Fetches the companies of present titles that have none yet. Stops when TMDB refuses the token
/// or asks for a long pause; other failures skip a title until the next run. Returns how many
/// titles got theirs.
pub async fn refresh_companies(pool: &SqlitePool, tmdb: &tmdb::Client) -> anyhow::Result<usize> {
    let mut done = 0;
    for (table, titles, tv) in [
        ("tmdb_movies", "movies", false),
        ("tmdb_series", "series", true),
    ] {
        let due: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT DISTINCT t.tmdb_id FROM {table} t JOIN {titles} x ON x.match_tmdb_id = t.tmdb_id
             WHERE x.missing_since IS NULL AND t.companies IS NULL LIMIT ?"
        )))
        .bind(BATCH)
        .fetch_all(pool)
        .await?;
        for id in due {
            let found = if tv {
                tmdb.series(id)
                    .await
                    .map(|d| d.map(|d| store::companies(&d.production_companies, &d.networks)))
            } else {
                tmdb.movie(id)
                    .await
                    .map(|d| d.map(|d| store::companies(&d.production_companies, &[])))
            };
            match found {
                Ok(companies) => {
                    // A title TMDB no longer has keeps an empty list, so it isn't asked again.
                    let companies = companies.transpose()?.unwrap_or_else(|| "[]".into());
                    sqlx::query(sqlx::AssertSqlSafe(format!(
                        "UPDATE {table} SET companies = ? WHERE tmdb_id = ?"
                    )))
                    .bind(companies)
                    .bind(id)
                    .execute(pool)
                    .await?;
                    done += 1;
                }
                Err(err) => skip(
                    err,
                    "companies",
                    &id.to_string(),
                    &mut RefreshReport::default(),
                )?,
            }
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tmdb::testing::{FakeTmdb, Reply};

    #[tokio::test]
    async fn titles_fetched_before_get_their_companies_once() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'm', 'movies', '/m'),
                 (2, 's', 'shows', '/s');
             INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES (603, 'The Matrix', 1);
             INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (1399, 'Show', 1);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_tmdb_id) VALUES (1, 1, 'M', 'M', 1, 'matched', 603);
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_tmdb_id) VALUES (1, 2, 'S', 'S', 1, 'matched', 1399);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let fake = FakeTmdb::default();
        fake.reply(
            "/movie/603",
            vec![Reply::ok(json!({ "id": 603, "title": "The Matrix",
                "production_companies": [{ "name": "Warner Bros. Pictures" },
                    { "name": "Village Roadshow Pictures" }] }))],
        );
        fake.reply(
            "/tv/1399?append_to_response=external_ids",
            vec![Reply::ok(json!({ "id": 1399, "name": "Show",
                "production_companies": [{ "name": "Small Films" }],
                "networks": [{ "name": "HBO" }, { "name": "Small Films" }] }))],
        );
        let tmdb = fake.start().await;
        assert_eq!(refresh_companies(&pool, &tmdb).await.unwrap(), 2);
        let movie: String =
            sqlx::query_scalar("SELECT companies FROM tmdb_movies WHERE tmdb_id = 603")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            movie,
            r#"["Warner Bros. Pictures","Village Roadshow Pictures"]"#
        );
        let series: String =
            sqlx::query_scalar("SELECT companies FROM tmdb_series WHERE tmdb_id = 1399")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(series, r#"["Small Films","HBO"]"#);
        assert_eq!(refresh_companies(&pool, &tmdb).await.unwrap(), 0, "once");
    }
}
