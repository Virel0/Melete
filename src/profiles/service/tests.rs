use sqlx::migrate::Migrator;

use super::*;

/// A database migrated up to, not including, the migration that gave profiles their service.
async fn before_services() -> (SqlitePool, Migrator) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().in_memory(true))
        .await
        .unwrap();
    let all = sqlx::migrate!();
    let before = Migrator {
        migrations: all
            .migrations
            .iter()
            .take_while(|m| m.description != "profile services")
            .cloned()
            .collect(),
        ..Migrator::DEFAULT
    };
    assert!(before.migrations.len() < all.migrations.len());
    before.run(&pool).await.unwrap();
    (pool, all)
}

#[tokio::test]
async fn a_profile_both_sides_use_is_split_and_each_series_keeps_its_choice() {
    let (pool, all) = before_services().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path, category)
             VALUES (1, 'Shows', 'shows', '/s', NULL), (2, 'Anime', 'shows', '/a', 'anime');
         INSERT INTO custom_formats (id, source, name, specifications, updated_at)
             VALUES (1, 'sonarr', 'x265', '[]', 0);
         INSERT INTO quality_profiles (id, source, name, upgrade_allowed, cutoff, min_format_score,
             cutoff_format_score, min_upgrade_format_score, language, updated_at, origin)
         VALUES (1, 'sonarr', 'HD-1080p', 1, 'WEBDL-1080p', 0, 0, 1, NULL, 0, 'imported'),
                (2, 'sonarr', 'Anime', 1, 'WEBDL-1080p', 0, 0, 1, NULL, 0, 'melete'),
                (3, 'sonarr', 'Only anime', 1, 'WEBDL-1080p', 0, 0, 1, NULL, 0, 'imported'),
                (4, 'sonarr', 'Unused', 1, 'WEBDL-1080p', 0, 0, 1, NULL, 0, 'imported'),
                (5, 'radarr', 'HD-1080p', 1, 'WEBDL-1080p', 0, 0, 1, 'en', 0, 'imported');
         INSERT INTO quality_profile_qualities (profile_id, quality, rank, group_name, allowed)
             VALUES (1, 'WEBDL-1080p', 0, NULL, 1), (1, 'Bluray-1080p', 1, NULL, 0);
         INSERT INTO quality_profile_formats (profile_id, format_id, score) VALUES (1, 1, 50);
         INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, library_id,
             quality_profile, profile_id, source, added_at)
         VALUES (1, 11, 'Severance', 1, 'standard', 1, NULL, 1, 'manual', 0),
                (2, 12, 'Frieren', 1, 'anime', 1, NULL, 1, 'manual', 0),
                (3, 13, 'Mushishi', 1, 'standard', 2, 'Only anime', NULL, 'sonarr', 0),
                (4, 14, 'Dandadan', 1, 'anime', 1, 'HD-1080p', NULL, 'sonarr', 0);
         INSERT INTO request_defaults (kind, library_id, profile_id)
             VALUES ('series', 1, 1), ('anime', 2, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    all.run(&pool).await.unwrap();

    let profiles: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, service, name FROM quality_profiles ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let copy = profiles
        .iter()
        .find(|(_, service, name)| service == "anime" && name == "HD-1080p")
        .expect("Anime's copy of the shared profile")
        .0;
    assert_eq!(
        profiles,
        [
            (1, "series".into(), "HD-1080p".into()),
            (2, "anime".into(), "Anime".into()),
            (3, "anime".into(), "Only anime".into()),
            (4, "series".into(), "Unused".into()),
            (5, "movies".into(), "HD-1080p".into()),
            (copy, "anime".into(), "HD-1080p".into()),
        ]
    );
    // The copy has the original's qualities, in order, and scores.
    let qualities = |id: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (String, i64, bool)>(
                "SELECT quality, rank, allowed FROM quality_profile_qualities
                 WHERE profile_id = ? ORDER BY rank",
            )
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(qualities(copy).await, qualities(1).await);
    let score: i64 =
        sqlx::query_scalar("SELECT score FROM quality_profile_formats WHERE profile_id = ?")
            .bind(copy)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(score, 50);

    // Severance keeps Series' profile, Frieren has Anime's copy, and the anime series that name
    // a Sonarr profile find Anime's by that name.
    let chosen: Vec<Option<i64>> =
        sqlx::query_scalar("SELECT profile_id FROM wanted_series ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(chosen, [Some(1), Some(copy), None, None]);
    assert_eq!(
        by_name(&pool, "anime", "Only anime").await.unwrap(),
        Some(3)
    );
    assert_eq!(
        by_name(&pool, "anime", "HD-1080p").await.unwrap(),
        Some(copy)
    );
    let defaults: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT kind, profile_id FROM request_defaults ORDER BY kind")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        defaults,
        [("anime".into(), Some(copy)), ("series".into(), Some(1))]
    );
}

#[tokio::test]
async fn sonarr_profiles_are_imported_where_their_series_are() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, quality_profile,
             source, added_at)
         VALUES (1, 11, 'Frieren', 1, 'anime', 'Any', 'sonarr', 0),
                (2, 12, 'Severance', 1, 'standard', 'Any', 'sonarr', 0),
                (3, 13, 'Dandadan', 1, 'anime', 'HD-1080p', 'sonarr', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let mut services = Vec::new();
    for name in ["Any", "HD-1080p", "Ultra-HD"] {
        services.push(import_services(&mut tx, App::Sonarr, name).await.unwrap());
    }
    assert_eq!(
        services,
        [
            vec!["series".to_owned(), "anime".into()],
            vec!["anime".into()],
            vec!["series".into()],
        ]
    );
    assert_eq!(
        import_services(&mut tx, App::Radarr, "Any").await.unwrap(),
        ["movies"]
    );
}

#[tokio::test]
async fn a_name_sonarr_gives_an_anime_series_is_animes_profile_else_series() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO quality_profiles (id, source, service, name, upgrade_allowed, cutoff,
             min_format_score, cutoff_format_score, min_upgrade_format_score, updated_at)
         VALUES (1, 'sonarr', 'series', 'HD-1080p', 1, 'WEBDL-1080p', 0, 0, 1, 0),
                (2, 'sonarr', 'series', 'Any', 1, 'WEBDL-1080p', 0, 0, 1, 0),
                (3, 'sonarr', 'anime', 'Any', 1, 'WEBDL-1080p', 0, 0, 1, 0),
                (4, 'radarr', NULL, 'Films', 1, 'WEBDL-1080p', 0, 0, 1, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(named(&pool, "anime", "Any").await.unwrap(), Some(3));
    assert_eq!(named(&pool, "anime", "HD-1080p").await.unwrap(), Some(1));
    assert_eq!(named(&pool, "series", "Any").await.unwrap(), Some(2));
    assert_eq!(named(&pool, "series", "Nothing").await.unwrap(), None);
    // A row written without a service gets its source's.
    assert_eq!(
        of_profile(&pool, 4).await.unwrap().as_deref(),
        Some("movies")
    );
}
