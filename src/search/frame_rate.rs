//! Movie releases converted to a high frame rate: a film shot at 24 frames a second, made up to
//! 48, 50, 60 or 120 by inventing the frames between. The extra frames are guesses, and some
//! players (Apple's among them) don't keep up with them, so the film stutters. Their names say
//! so (`60fps`, `interpolated`), and a movie's search turns them down unless an admin allows
//! them, for every film or for one (`high_frame_rate_films`, starting with the films really
//! made at a high frame rate). Series are left alone: 50 and 59.94 frames a second are normal
//! for television.

use std::sync::LazyLock;

use regex::Regex;
use sqlx::SqlitePool;

/// On, a movie release named as converted to a high frame rate is taken like any other.
pub(crate) const ALLOW_SETTING: &str = "MELETE_ALLOW_HFR_MOVIES";

/// A name's mark of a converted high frame rate: 48, 50, 60 or 120 fps (with or without a
/// separator), or "interpolated".
static CONVERTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:(48|50|60|120)[ ._-]?fps|interpolated)(?:$|[^a-z0-9])")
        .expect("the pattern is valid")
});

/// Whether a movie's search turns these releases down: unless an admin allows them for every
/// film, or for this one.
pub(super) async fn turned_down(pool: &SqlitePool, tmdb_id: Option<i64>) -> sqlx::Result<bool> {
    if let Some(id) = tmdb_id
        && allowed(pool, id).await?
    {
        return Ok(false);
    }
    Ok(crate::settings::value(pool, ALLOW_SETTING)
        .await?
        .as_deref()
        != Some("on"))
}

/// Whether high frame rate releases are wanted for the film with TMDB id `tmdb_id`.
pub(crate) async fn allowed(pool: &SqlitePool, tmdb_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM high_frame_rate_films WHERE tmdb_id = ?)")
        .bind(tmdb_id)
        .fetch_one(pool)
        .await
}

/// The films high frame rate releases are wanted for, by TMDB id, in order.
pub(crate) async fn allowed_films(pool: &SqlitePool) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT tmdb_id FROM high_frame_rate_films ORDER BY tmdb_id")
        .fetch_all(pool)
        .await
}

/// Wants high frame rate releases for the film with TMDB id `tmdb_id`, or stops wanting them.
pub(crate) async fn allow(pool: &SqlitePool, tmdb_id: i64, on: bool) -> sqlx::Result<()> {
    if on {
        sqlx::query(
            "INSERT INTO high_frame_rate_films (tmdb_id, added_at) VALUES (?, ?)
             ON CONFLICT (tmdb_id) DO NOTHING",
        )
        .bind(tmdb_id)
        .bind(crate::db::unix_now())
        .execute(pool)
        .await?;
    } else {
        sqlx::query("DELETE FROM high_frame_rate_films WHERE tmdb_id = ?")
            .bind(tmdb_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// A frame rate this high, for a film, was almost surely made up from its own 24: 48 and up.
pub(crate) const LIKELY_CONVERTED_FPS: f64 = 47.5;

/// A movie in the library playing at a high frame rate.
#[derive(Debug, PartialEq, sqlx::FromRow)]
pub(crate) struct HighFrameRate {
    /// The library's movie.
    pub id: i64,
    /// Its title.
    pub title: String,
    /// The highest of its present files' frame rates (their first video stream's), as probed.
    pub fps: f64,
    /// Whether high frame rate releases are wanted for the film, so it isn't one made up.
    pub allowed: bool,
}

/// Movies in the library playing at 48 fps or more, by title; or only `movie`.
pub(crate) async fn high_frame_rate_movies(
    pool: &SqlitePool,
    movie: Option<i64>,
) -> sqlx::Result<Vec<HighFrameRate>> {
    sqlx::query_as(
        "SELECT m.id, COALESCE(t.title, m.parsed_title) AS title,
             max(json_extract(p.info, '$.video[0].frame_rate')) AS fps,
             EXISTS (SELECT 1 FROM high_frame_rate_films h WHERE h.tmdb_id = m.match_tmdb_id)
                 AS allowed
         FROM movies m
         JOIN media_files f ON f.movie_id = m.id AND f.missing_since IS NULL
         JOIN media_probes p ON p.file_id = f.id AND p.info IS NOT NULL
         LEFT JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id
         WHERE m.missing_since IS NULL AND (?1 IS NULL OR m.id = ?1)
         GROUP BY m.id HAVING fps >= ?2
         ORDER BY title, m.id",
    )
    .bind(movie)
    .bind(LIKELY_CONVERTED_FPS)
    .fetch_all(pool)
    .await
}

/// Why a movie release is turned down for its name marking a converted high frame rate, or
/// None when it doesn't.
pub(super) fn rejection(name: &str) -> Option<String> {
    let found = CONVERTED.captures(name)?;
    let what = match found.get(1) {
        Some(rate) => format!("{} fps", rate.as_str()),
        None => "interpolated".to_owned(),
    };
    Some(format!(
        "converted to a high frame rate ({what}): its extra frames are made up, and it may \
         stutter on some players"
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn names_marking_a_converted_high_frame_rate_are_found() {
        for (name, what) in [
            ("300.2006.2160p.60fps.HEVC.Main10.SDR", "60 fps"),
            ("300 2006 2160p 60FPS HEVC", "60 fps"),
            ("Film.2019.1080p.60.fps.x264", "60 fps"),
            ("Film.2019.1080p.48fps-GROUP", "48 fps"),
            ("Film.2019.2160p.120-fps.WEB", "120 fps"),
            ("Film.2019.1080p.50fps", "50 fps"),
            ("Film.2019.1080p.Interpolated.x265", "interpolated"),
        ] {
            let why = rejection(name).unwrap_or_else(|| panic!("{name}"));
            assert!(why.contains(&format!("({what})")), "{name}: {why}");
        }
    }

    #[test]
    fn other_names_are_left_alone() {
        for name in [
            "300.2006.2160p.UHD.BluRay.HEVC",
            "Film.2019.1080p.24fps.x264",
            "Film.2019.1080p.25fps",
            "Film.2019.1080p.23.976fps",
            "Film.2019.HFR.2160p",
            // Numbers that are part of something else.
            "Film.2060fps.2019",
            "Film.2019.1080p.H264.AAC5.1-160fps2",
            "Interpolation.Of.Dreams.2019.1080p",
        ] {
            assert_eq!(rejection(name), None, "{name}");
        }
    }

    #[tokio::test]
    async fn turned_down_unless_allowed_or_really_made_so() {
        let pool = crate::db::open_in_memory().await;
        assert!(turned_down(&pool, Some(1)).await.unwrap());
        assert!(turned_down(&pool, None).await.unwrap());
        // The films really made at a high frame rate, as the list starts.
        let genuine = allowed_films(&pool).await.unwrap();
        assert_eq!(genuine, [49051, 57158, 122_917, 293_767, 453_405]);
        for film in genuine {
            assert!(!turned_down(&pool, Some(film)).await.unwrap(), "{film}");
        }
        // One an admin adds, and one taken off.
        allow(&pool, 1, true).await.unwrap();
        assert!(!turned_down(&pool, Some(1)).await.unwrap());
        allow(&pool, 1, false).await.unwrap();
        allow(&pool, 49051, false).await.unwrap();
        assert!(turned_down(&pool, Some(1)).await.unwrap());
        assert!(turned_down(&pool, Some(49051)).await.unwrap());
        sqlx::query("INSERT INTO settings (name, value, updated_at) VALUES (?, 'on', 0)")
            .bind(ALLOW_SETTING)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!turned_down(&pool, Some(1)).await.unwrap());
    }

    /// Movies 1 to 6: at 60 fps; at 23.976; at 25; the first Hobbit film (really made at 48)
    /// at 48; unmatched at 60; at 24 with a copy at 60 that's gone from disk.
    pub(crate) async fn movies_at_frame_rates(pool: &SqlitePool) {
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES
                 (1001, 'Sixty', 0), (1002, 'Film', 0), (1003, 'Pal', 0), (49051, 'Hobbit', 0),
                 (1006, 'Gone Sixty', 0);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 match_status, match_tmdb_id) VALUES
                 (1, 1, 'A', 'Sixty', 1, 'matched', 1001),
                 (2, 1, 'B', 'Film', 1, 'matched', 1002),
                 (3, 1, 'C', 'Pal', 1, 'matched', 1003),
                 (4, 1, 'D', 'Hobbit', 1, 'matched', 49051),
                 (5, 1, 'E', 'Unmatched Sixty', 1, NULL, NULL),
                 (6, 1, 'F', 'Gone Sixty', 1, 'matched', 1006);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation, missing_since) VALUES
                 (1, 1, 'A/a.mkv', 1, 1, 1, 1, 1, NULL), (2, 1, 'B/b.mkv', 1, 1, 2, 1, 1, NULL),
                 (3, 1, 'C/c.mkv', 1, 1, 3, 1, 1, NULL), (4, 1, 'D/d.mkv', 1, 1, 4, 1, 1, NULL),
                 (5, 1, 'E/e.mkv', 1, 1, 5, 1, 1, NULL), (6, 1, 'F/f.mkv', 1, 1, 6, 1, 1, NULL),
                 (7, 1, 'F/old.mkv', 1, 1, 6, 1, 1, 5);
             INSERT INTO media_probes (file_id, size, modified_at, probed_at, info) VALUES
                 (1, 1, 1, 1, '{"video":[{"frame_rate":60.0}]}'),
                 (2, 1, 1, 1, '{"video":[{"frame_rate":23.976}]}'),
                 (3, 1, 1, 1, '{"video":[{"frame_rate":25.0}]}'),
                 (4, 1, 1, 1, '{"video":[{"frame_rate":48.0}]}'),
                 (5, 1, 1, 1, '{"video":[{"frame_rate":60.0}]}'),
                 (6, 1, 1, 1, '{"video":[{"frame_rate":24.0}]}'),
                 (7, 1, 1, 1, '{"video":[{"frame_rate":60.0}]}');"#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn movies_at_a_high_frame_rate_are_found_saying_whether_theyre_wanted_so() {
        let pool = crate::db::open_in_memory().await;
        movies_at_frame_rates(&pool).await;
        let high = |id, title: &str, fps, allowed| HighFrameRate {
            id,
            title: title.to_owned(),
            fps,
            allowed,
        };
        // Not the 23.976 or 25 fps films, nor the one whose 60 fps copy is gone.
        assert_eq!(
            high_frame_rate_movies(&pool, None).await.unwrap(),
            [
                high(4, "Hobbit", 48.0, true),
                high(1, "Sixty", 60.0, false),
                high(5, "Unmatched Sixty", 60.0, false),
            ]
        );
        assert_eq!(high_frame_rate_movies(&pool, Some(2)).await.unwrap(), []);
        assert_eq!(
            high_frame_rate_movies(&pool, Some(1)).await.unwrap(),
            [high(1, "Sixty", 60.0, false)]
        );
        // An admin's choice shows at once.
        allow(&pool, 1001, true).await.unwrap();
        assert!(high_frame_rate_movies(&pool, Some(1)).await.unwrap()[0].allowed);
    }
}
