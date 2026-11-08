//! A music result's picture: an album's cover in the library, found here. Covers of music not
//! in the library are the API's to offer, only with that setting on, from the release group
//! [`cover_group`] picks ([`crate::search_covers`]); a movie's or series' poster is the API's to
//! give, as discovery gives posters (`tmdb_poster`).

use super::*;

/// The address of its cover on this server, for an album in the library.
pub(super) async fn here(pool: &SqlitePool, c: &Candidate) -> sqlx::Result<Option<String>> {
    if c.kind != "album" || !c.in_library {
        return Ok(None);
    }
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM music_albums WHERE missing_since IS NULL
           AND coalesce(match_release_group_id, mb_release_group_id) = ?
         ORDER BY id LIMIT 1",
    )
    .bind(&c.mb_release_group_id)
    .fetch_optional(pool)
    .await?;
    Ok(id.map(|id| format!("/api/v1/music/albums/{id}/cover")))
}

/// The release group whose cover a song takes, from the releases MusicBrainz's search lists it
/// on: its single's, else its album's; of each, a plain one (no secondary type, such as
/// Compilation or Live) before the others, as listed. None when it's on neither.
pub(super) fn cover_group(releases: &[musicbrainz::RecordingRelease]) -> Option<String> {
    let groups: Vec<&musicbrainz::ReleaseGroup> = releases
        .iter()
        .filter_map(|r| r.release_group.as_ref())
        .collect();
    let plain = |g: &&&musicbrainz::ReleaseGroup| {
        g.secondary_types.as_deref().unwrap_or_default().is_empty()
    };
    ["Single", "Album"].into_iter().find_map(|kind| {
        let of_kind = || {
            groups
                .iter()
                .filter(move |g| g.primary_type.as_deref() == Some(kind))
        };
        of_kind()
            .find(plain)
            .or_else(|| of_kind().next())
            .map(|g| g.id.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicbrainz::{RecordingRelease, ReleaseGroup};

    fn on(id: &str, primary: &str, secondary: &[&str]) -> RecordingRelease {
        RecordingRelease {
            title: id.into(),
            status: Some("Official".into()),
            release_group: Some(ReleaseGroup {
                id: id.into(),
                title: Some(id.into()),
                primary_type: Some(primary.into()),
                secondary_types: Some(secondary.iter().map(|s| s.to_string()).collect()),
                first_release_date: None,
            }),
        }
    }

    #[test]
    fn a_song_takes_its_singles_cover_else_its_albums() {
        let album = on("album", "Album", &[]);
        let best_of = on("best-of", "Album", &["Compilation"]);
        let single = on("single", "Single", &[]);
        let live_single = on("live-single", "Single", &["Live"]);
        let ep = on("ep", "EP", &[]);
        let group = |releases: &[RecordingRelease]| cover_group(releases);
        assert_eq!(
            group(&[best_of.clone(), album.clone(), live_single.clone(), single]).as_deref(),
            Some("single")
        );
        assert_eq!(
            group(&[album.clone(), live_single]).as_deref(),
            Some("live-single")
        );
        assert_eq!(group(&[best_of.clone(), album]).as_deref(), Some("album"));
        assert_eq!(group(&[best_of]).as_deref(), Some("best-of"));
        assert_eq!(group(&[ep]), None, "neither a single nor an album");
        let unknown = RecordingRelease {
            title: "x".into(),
            status: None,
            release_group: None,
        };
        assert_eq!(group(&[unknown]), None);
    }
}
