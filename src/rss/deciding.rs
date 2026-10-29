//! Deciding on a sync's new releases as searches' results are, and keeping them as searches of
//! that sync.

use super::*;

/// Decides on each title's new releases, refusing those with a reason found here, and keeps
/// them; a title without a profile is skipped. Each kept search, with its target and candidate.
pub(super) async fn decide_titles(
    pool: &SqlitePool,
    tmdb: Option<&crate::tmdb::Client>,
    candidates: &[Candidate],
    groups: &[Group<'_>],
    sync_id: i64,
    summary: &[&IndexerOutcome],
) -> anyhow::Result<Vec<(Target, usize, i64)>> {
    let mut stored: Vec<(Target, usize, i64)> = Vec::new();
    for Group {
        target,
        candidate,
        releases,
    } in groups
    {
        let wanted = &candidates[*candidate].wanted;
        let profile = match search::profile_for(pool, *target, wanted).await? {
            Ok(profile) => profile,
            Err(refused) => {
                tracing::debug!(?target, reason = %refused, "RSS releases for a title without a profile");
                continue;
            }
        };
        let items: Vec<(i64, &torznab::Item)> =
            releases.iter().map(|(i, item, _)| (*i, *item)).collect();
        let mut decided = search::decide_all(pool, tmdb, wanted, &profile, *target, &items).await?;
        for ((_, _, decision), (_, _, why)) in decided.iter_mut().zip(releases) {
            if let Some(why) = why {
                decision.rejections.insert(0, why.clone());
                decision.accepted = false;
            }
        }
        let search_id = search::store(
            pool,
            *target,
            profile.profile.id,
            Some(sync_id),
            summary,
            &decided,
        )
        .await?;
        stored.push((*target, *candidate, search_id));
    }
    Ok(stored)
}

/// Decides on each album's new releases, refusing those with a reason found here, and keeps
/// them; an album whose artist has no profile is skipped. Each kept search, with its album.
pub(super) async fn decide_albums(
    pool: &SqlitePool,
    albums: &[album_search::WantedAlbum],
    album_groups: &[(usize, Found<'_>)],
    sync_id: i64,
    summary: &[&IndexerOutcome],
) -> anyhow::Result<Vec<(i64, i64)>> {
    let mut stored_albums: Vec<(i64, i64)> = Vec::new();
    for (n, releases) in album_groups {
        let album = &albums[*n];
        let Some(profile) = music_profiles::for_artist(pool, album.wanted_artist_id).await? else {
            tracing::debug!(
                album = album.id,
                "RSS releases for an album without a profile"
            );
            continue;
        };
        let decided: Vec<(i64, &torznab::Item, album_search::AlbumDecision)> = releases
            .iter()
            .map(|(indexer_id, item, why)| {
                let mut decision =
                    album_search::decide(&item.title, &album.title, &album.artist, &profile);
                if let Some(why) = why {
                    decision.reasons.insert(0, why.clone());
                    decision.accepted = false;
                }
                (*indexer_id, *item, decision)
            })
            .collect();
        let search_id = album_search::store(
            pool,
            album.id,
            profile.id,
            Some(sync_id),
            summary,
            &decided,
            None,
        )
        .await?;
        stored_albums.push((album.id, search_id));
    }
    Ok(stored_albums)
}
