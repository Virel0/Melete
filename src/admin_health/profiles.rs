//! Profiles for requests: a service with several quality profiles and none set for its approved
//! requests, which then go without one and aren't grabbed.

use super::*;

/// Each video service whose requests have no default profile while it has several to choose
/// from, as a note in its own area: one approved without a choice gets none (with one, it
/// gets that one).
pub(super) async fn requests_without_profile(
    pool: &SqlitePool,
    found: &mut Found,
) -> anyhow::Result<()> {
    for (service, name) in [
        ("movies", "Movies"),
        ("series", "Series"),
        ("anime", "Anime"),
    ] {
        let profiles = crate::profiles::service::count(pool, service).await?;
        if profiles < 2 {
            continue;
        }
        let set: Option<i64> = sqlx::query_scalar(
            "SELECT profile_id FROM request_defaults WHERE kind = ? AND profile_id IS NOT NULL",
        )
        .bind(service)
        .fetch_optional(pool)
        .await?;
        if set.is_some() {
            continue;
        }
        found.add(
            service,
            Level::Note,
            "requests_no_profile",
            format!(
                "Requests for {service} have no default profile, so one approved without choosing one isn't grabbed; set one in Manage → {name} → Requests → Where they go"
            ),
            profiles,
            None,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The requests_no_profile notes, as (area, level, count, message).
    fn notes(health: Health) -> Vec<(&'static str, Level, i64, String)> {
        health
            .areas
            .into_iter()
            .flat_map(|a| {
                a.findings
                    .into_iter()
                    .filter(|f| f.code == "requests_no_profile")
                    .map(move |f| (a.area, f.level, f.count, f.message))
            })
            .collect()
    }

    #[tokio::test]
    async fn several_profiles_and_none_set_for_requests_is_a_note() {
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO quality_profiles (id, source, service, name, upgrade_allowed, cutoff,
                 min_format_score, cutoff_format_score, min_upgrade_format_score, updated_at)
             VALUES (1, 'radarr', 'movies', 'HD', 1, 'Bluray-1080p', 0, 0, 1, 0),
                    (2, 'radarr', 'movies', 'UHD', 1, 'Bluray-2160p', 0, 0, 1, 0),
                    (3, 'sonarr', 'series', 'HD', 1, 'Bluray-1080p', 0, 0, 1, 0),
                    (4, 'sonarr', 'anime', 'HD', 1, 'Bluray-1080p', 0, 0, 1, 0),
                    (5, 'sonarr', 'anime', 'UHD', 1, 'Bluray-2160p', 0, 0, 1, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Series has only one, which its requests get; Movies and Anime have a choice to make.
        let found = notes(check(&pool).await.unwrap());
        assert_eq!(
            found
                .iter()
                .map(|(a, l, n, _)| (*a, *l, *n))
                .collect::<Vec<_>>(),
            [("movies", Level::Note, 2), ("anime", Level::Note, 2)]
        );
        assert_eq!(
            found[0].3,
            "Requests for movies have no default profile, so one approved without choosing one isn't grabbed; set one in Manage → Movies → Requests → Where they go"
        );

        // Setting one answers it; a row with no profile in it doesn't.
        sqlx::query(
            "INSERT INTO request_defaults (kind, library_id, profile_id)
             VALUES ('movies', NULL, 2), ('anime', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let found = notes(check(&pool).await.unwrap());
        assert_eq!(
            found.iter().map(|(a, ..)| *a).collect::<Vec<_>>(),
            ["anime"]
        );
    }
}
