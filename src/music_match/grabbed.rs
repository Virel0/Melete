//! Albums Melete imported for a wanted album: matched within the release group they were
//! grabbed for, choosing the edition whose track count fits, rather than by their own tags or
//! names. Only an album folder that one wanted album's imported downloads went into.

use super::*;

/// The wanted album an imported album folder was grabbed for: its release group, and its title
/// and artist as MusicBrainz names them.
pub(super) struct Grabbed {
    group: String,
    title: String,
    artist: String,
}

/// What album `id` was grabbed for, when exactly one wanted album's imported downloads went
/// into its folder.
pub(super) async fn grabbed_for(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Grabbed>> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT min(w.mb_release_group_id), min(w.title), min(ar.name) FROM music_albums a
         JOIN libraries l ON l.id = a.library_id
         JOIN downloads d ON d.state = 'imported' AND d.wanted_album_id IS NOT NULL
             AND d.destination = rtrim(l.path, '/') || '/' || a.folder
         JOIN wanted_albums w ON w.id = d.wanted_album_id
         JOIN wanted_artists ar ON ar.id = w.wanted_artist_id
         WHERE a.id = ?
         GROUP BY a.id HAVING count(DISTINCT w.id) = 1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(group, title, artist)| Grabbed {
        group,
        title,
        artist,
    }))
}

/// The release in the group the album was grabbed for: searched for by the wanted album's title
/// and artist, the editions in that group only, the one whose track count fits first (then an
/// official one, then the earliest), as matching chooses. None when the search finds no edition
/// of the group, for normal matching to decide.
pub(super) async fn decide_grabbed(
    mb: &musicbrainz::Client,
    album: &Album,
    grabbed: &Grabbed,
) -> Result<Option<Outcome>, musicbrainz::Error> {
    let found = mb.search(&grabbed.title, Some(&grabbed.artist)).await?;
    let best = found
        .iter()
        .filter(|f| f.release_group.id == grabbed.group)
        .min_by_key(|f| {
            (
                f.track_count != Some(album.tracks),
                f.status.as_deref() != Some("Official"),
                f.date
                    .clone()
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| "9999".into()),
            )
        });
    let Some(best) = best else {
        return Ok(None);
    };
    Ok(Some(Outcome::Matched {
        release: Box::new(mb.release(&best.id).await?),
        method: "search",
    }))
}
