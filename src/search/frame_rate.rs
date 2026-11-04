//! Movie releases converted to a high frame rate: a film shot at 24 frames a second, made up to
//! 48, 50, 60 or 120 by inventing the frames between. The extra frames are guesses, and some
//! players (Apple's among them) don't keep up with them, so the film stutters. Their names say
//! so (`60fps`, `interpolated`), and a movie's search turns them down unless an admin allows
//! them. Series are left alone: 50 and 59.94 frames a second are normal for television.

use std::sync::LazyLock;

use regex::Regex;
use sqlx::SqlitePool;

/// On, a movie release named as converted to a high frame rate is taken like any other.
pub(crate) const ALLOW_SETTING: &str = "MELETE_ALLOW_HFR_MOVIES";

/// Films really made at a high frame rate, by TMDB id, whose high frame rate releases are
/// genuine: Billy Lynn's Long Halftime Walk, Gemini Man, and the three Hobbit films.
pub(crate) const MADE_AT_HIGH_FRAME_RATE: [i64; 5] = [293_767, 453_405, 49_051, 57_158, 122_917];

/// A name's mark of a converted high frame rate: 48, 50, 60 or 120 fps (with or without a
/// separator), or "interpolated".
static CONVERTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:(48|50|60|120)[ ._-]?fps|interpolated)(?:$|[^a-z0-9])")
        .expect("the pattern is valid")
});

/// Whether a movie's search turns these releases down: unless an admin allows them, or the
/// film is one really made at a high frame rate.
pub(super) async fn turned_down(pool: &SqlitePool, tmdb_id: Option<i64>) -> sqlx::Result<bool> {
    if tmdb_id.is_some_and(|id| MADE_AT_HIGH_FRAME_RATE.contains(&id)) {
        return Ok(false);
    }
    Ok(crate::settings::value(pool, ALLOW_SETTING)
        .await?
        .as_deref()
        != Some("on"))
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
mod tests {
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
        for genuine in MADE_AT_HIGH_FRAME_RATE {
            assert!(
                !turned_down(&pool, Some(genuine)).await.unwrap(),
                "{genuine}"
            );
        }
        sqlx::query("INSERT INTO settings (name, value, updated_at) VALUES (?, 'on', 0)")
            .bind(ALLOW_SETTING)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!turned_down(&pool, Some(1)).await.unwrap());
    }
}
