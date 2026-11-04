//! Movies playing at a frame rate likely made up from the film's own: a note for each in
//! Movies, as such a copy may stutter on some players and another release is worth a search.
//! Films an admin wants high frame rate releases for aren't noted.

use sqlx::SqlitePool;

use super::{Found, Level};

/// A note for each movie whose copy plays at 48 fps or more, unless high frame rate releases
/// are wanted for the film: an admin chose those.
pub(super) async fn converted_frame_rates(
    pool: &SqlitePool,
    found: &mut Found,
) -> anyhow::Result<()> {
    let movies = crate::search::frame_rate::high_frame_rate_movies(pool, None).await?;
    for movie in movies.into_iter().filter(|m| !m.allowed) {
        found.add(
            "movies",
            Level::Note,
            "movies_high_frame_rate",
            format!(
                "{} plays at {} fps, likely made up from the film's 24; it may stutter on some \
                 players. Search for another release.",
                movie.title,
                movie.fps.round()
            ),
            1,
            Some(movie.id),
        );
    }
    Ok(())
}
