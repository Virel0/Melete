use super::*;

/// Anime in an anime library and anime wanted as such in another, with episodes carrying ASS
/// in the file, beside it, or not at all; and a series that isn't anime.
pub(crate) async fn anime_episodes(pool: &SqlitePool, anime: &Path, shows: &Path) {
    for (dir, name) in [
        (
            anime,
            "Frieren/Season 1/[SubsPlease] Frieren - S01E03 (1080p).ass",
        ),
        (
            anime,
            "Frieren/Season 1/[SubsPlease] Frieren - S01E03 (1080p).mkv",
        ),
        (
            anime,
            "Frieren/Season 1/[SubsPlease] Frieren - S01E02 (1080p).en.srt",
        ),
    ] {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        r#"INSERT INTO libraries (id, name, kind, path, category) VALUES
             (1, 'anime', 'shows', '{anime}', 'anime'), (2, 'shows', 'shows', '{shows}', NULL);
         INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (2001, 'Frieren', 0);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES
             (1, 1, 'Frieren', 'Sousou no Frieren', 1, 'matched', 2001),
             (2, 2, 'Mushishi', 'Mushishi', 1, NULL, NULL),
             (3, 2, 'Drama', 'Drama', 1, NULL, NULL);
         INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, series_id,
             source, added_at) VALUES (1, 9, 'Mushishi', 1, 'anime', 2, 'manual', 0);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id, season,
             episode, first_seen_generation, seen_generation, missing_since) VALUES
             (1, 1, 'Frieren/Season 1/[SubsPlease] Frieren - S01E01 (1080p).mkv', 1, 1, 1, 1, 1, 1, 1, NULL),
             (2, 1, 'Frieren/Season 1/[SubsPlease] Frieren - S01E02 (1080p).mkv', 1, 1, 1, 1, 2, 1, 1, NULL),
             (3, 1, 'Frieren/Season 1/[SubsPlease] Frieren - S01E03 (1080p).mkv', 1, 1, 1, 1, 3, 1, 1, NULL),
             (4, 1, 'Frieren/Season 1/Frieren.S01E04.1080p.WEB-DL-RAW.mkv', 1, 1, 1, 1, 4, 1, 1, 9),
             (5, 1, 'Frieren/Specials/Frieren.S00E01.mkv', 1, 1, 1, 0, 1, 1, 1, NULL),
             (6, 2, 'Mushishi/Mushishi.S01E01.mkv', 1, 1, 2, 1, 1, 1, 1, NULL),
             (7, 2, 'Drama/Drama.S01E01.mkv', 1, 1, 3, 1, 1, 1, 1, NULL),
             (8, 1, 'Frieren/Season 1/Frieren.S01E05.1080p.WEB-DL-Unprobed.mkv', 1, 1, 1, 1, 5, 1, 1, NULL);
         INSERT INTO media_probes (file_id, size, modified_at, probed_at, info) VALUES
             (1, 1, 1, 1, '{{"subtitles":[{{"codec":"subrip"}},{{"codec":"ass"}}]}}'),
             (2, 1, 1, 1, '{{"subtitles":[{{"codec":"subrip"}}]}}'),
             (3, 1, 1, 1, '{{"subtitles":[]}}'),
             (4, 1, 1, 1, '{{"subtitles":[]}}'),
             (5, 1, 1, 1, '{{"subtitles":[]}}'),
             (6, 1, 1, 1, '{{}}'),
             (7, 1, 1, 1, '{{"subtitles":[]}}');"#,
        anime = anime.display(),
        shows = shows.display(),
    )))
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn anime_episodes_without_ass_in_the_file_or_beside_it_are_found() {
    let pool = crate::db::open_in_memory().await;
    let (anime, shows) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    anime_episodes(&pool, anime.path(), shows.path()).await;
    let found = |series, season, episode, group: Option<&str>, title: &str| Unstyled {
        series_id: series,
        series: title.to_owned(),
        file_id: if series == 1 { 2 } else { 6 },
        season,
        episode,
        group: group.map(str::to_owned),
    };
    // Episode 1 has an ASS track, 3 an ASS file beside it (2's SRT doesn't count), 4 is gone,
    // the special isn't an episode, 5 isn't probed yet, and the drama isn't anime.
    assert_eq!(
        unstyled(&pool, None).await.unwrap(),
        [
            found(1, 1, 2, Some("SubsPlease"), "Frieren"),
            found(2, 1, 1, None, "Mushishi"),
        ]
    );
    assert_eq!(
        unstyled(&pool, Some(2)).await.unwrap(),
        [found(2, 1, 1, None, "Mushishi")]
    );
    assert_eq!(unstyled(&pool, Some(3)).await.unwrap(), []);
}
