//! What ListenBrainz suggests (recordings alongside the seeds', and the most listened-to of the
//! artists alongside theirs), sorted into songs the library has (to play) and songs it hasn't
//! (to request), and the ranking that makes a list of them.

use std::collections::HashMap;

use super::*;
use crate::library::ARTIST_MBID;
use crate::listenbrainz::{Recording, SimilarRecording};
use crate::search_keys::fold;

/// The closest recordings taken from each seed recording's answer (ListenBrainz gives up to
/// 100): enough for 40 songs after the caps, few enough to look up in a few batches.
const PER_SEED_RECORDINGS: usize = 25;
/// The closest artists taken from each seed artist's answer.
const PER_SEED_ARTISTS: usize = 10;
/// The tracks considered of each artist alongside, by id: more than [`PER_ARTIST`], as some may
/// be left out.
const TRACKS_PER_ARTIST: usize = 5;
/// At most this many songs by one artist; at most one from one album.
const PER_ARTIST: usize = 2;
/// The artists alongside whose most listened-to recordings are asked for, closest first: each
/// is one request, so a list asks some 10 more.
const TOP_ARTISTS: usize = 10;
/// The most listened-to recordings taken of each: more than [`PER_ARTIST`], as some may be in
/// the library or left out.
pub(super) const TOP_PER_ARTIST: usize = 10;

/// Recordings suggested, by id: each one's best score, what ListenBrainz said of it, and the
/// seed that gave it.
type Suggested = HashMap<String, (f64, SimilarRecording, Because)>;

/// Keeps `r` as suggested with `score`, unless it's one of `mine` (the seeds' own) or was
/// suggested more strongly already.
fn suggest(
    best: &mut Suggested,
    mine: &HashSet<&str>,
    score: f64,
    r: SimilarRecording,
    because: &Because,
) {
    if mine.contains(r.recording_mbid.as_str()) {
        return;
    }
    if best
        .get(&r.recording_mbid)
        .is_none_or(|(was, ..)| score > *was)
    {
        best.insert(r.recording_mbid.clone(), (score, r, because.clone()));
    }
}

/// A song that could be on the list.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Candidate {
    /// The song, as the list keeps it.
    pub item: Item,
    /// How strongly it's suggested: ListenBrainz's score against the best in its answer, times
    /// how strongly its seed figures in the listener's plays. 0 to 1.
    pub score: f64,
    /// What ties are broken on: the track id or the recording.
    pub order: String,
    /// Its first artist, folded, for the per-artist cap.
    pub artist: String,
    /// Its album (release group, else the library's album), for the per-album cap.
    pub release: Option<String>,
}

/// The list: best first, ties broken on [`Candidate::order`], at most [`PER_ARTIST`] by one
/// artist and one from one album, [`ITEMS`] in all. Each kind (to play, to request) gets up to
/// half first; what's left goes to the best of either.
pub(super) fn rank(mut all: Vec<Candidate>) -> Vec<Item> {
    all.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.order.cmp(&b.order))
    });
    let mut taken = vec![false; all.len()];
    let mut per_artist: HashMap<&str, usize> = HashMap::new();
    let mut releases: HashSet<&str> = HashSet::new();
    let (mut kinds, mut total) = ([0usize; 2], 0);
    for share in [true, false] {
        for (i, c) in all.iter().enumerate() {
            let kind = usize::from(c.item.song.is_some());
            if total == ITEMS || taken[i] || (share && kinds[kind] >= ITEMS / 2) {
                continue;
            }
            if per_artist.get(c.artist.as_str()).copied().unwrap_or(0) >= PER_ARTIST
                || c.release.as_deref().is_some_and(|r| releases.contains(r))
            {
                continue;
            }
            taken[i] = true;
            kinds[kind] += 1;
            total += 1;
            *per_artist.entry(c.artist.as_str()).or_default() += 1;
            releases.extend(c.release.as_deref());
        }
    }
    all.into_iter()
        .zip(taken)
        .filter_map(|(c, taken)| taken.then_some(c.item))
        .collect()
}

/// The tracks `user` isn't offered to play: heard in the last 30 days, finished
/// [`PLAYED_OFTEN`] times, or in `flagged`.
async fn left_out(
    pool: &SqlitePool,
    user: i64,
    flagged: &HashSet<i64>,
    now: i64,
) -> sqlx::Result<HashSet<i64>> {
    let heard: Vec<i64> = sqlx::query_scalar(
        "SELECT track_id FROM music_plays WHERE user_id = ? AND track_id IS NOT NULL
         GROUP BY track_id HAVING max(started_at) >= ? OR sum(completed) >= ?",
    )
    .bind(user)
    .bind(now * 1000 - HEARD_LATELY_MS)
    .bind(PLAYED_OFTEN)
    .fetch_all(pool)
    .await?;
    Ok(heard.into_iter().chain(flagged.iter().copied()).collect())
}

/// The present track `access` sees of each of `recordings` that the library has, by recording.
async fn by_recording(
    pool: &SqlitePool,
    access: &Access,
    recordings: &[String],
) -> sqlx::Result<HashMap<String, i64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT coalesce(t.mb_recording_id, t.match_recording_id) AS mbid, min(t.id)
         FROM music_tracks t
         WHERE t.missing_since IS NULL AND {}
           AND coalesce(t.mb_recording_id, t.match_recording_id)
               IN (SELECT value FROM json_each(?))
         GROUP BY mbid",
        access.track_sql("t")
    )))
    .bind(serde_json::to_string(recordings).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The present tracks `access` sees crediting any of `artists` (MusicBrainz ids): each one's
/// id, title and that artist's id, by track id.
async fn by_artists(
    pool: &SqlitePool,
    access: &Access,
    artists: &HashSet<&str>,
) -> sqlx::Result<Vec<(i64, String, String)>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, title, mb_artist_id FROM (
             SELECT t.id, t.title, {ARTIST_MBID} FROM music_tracks t
             JOIN music_track_artists ta ON ta.track_id = t.id
             JOIN music_artists ar ON ar.id = ta.artist_id
             WHERE t.missing_since IS NULL AND {})
         WHERE mb_artist_id IN (SELECT value FROM json_each(?))
         ORDER BY id, mb_artist_id",
        access.track_sql("t")
    )))
    .bind(serde_json::to_string(artists).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await
}

/// The recordings alongside the seed recordings, each once with its best score and the seed
/// that gave it; none a seed itself.
async fn alike_recordings(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    seeds: &seeds::Seeds,
    now: i64,
) -> anyhow::Result<Suggested> {
    let mine: HashSet<&str> = seeds.recordings.iter().map(|s| s.mbid.as_str()).collect();
    let mut best = Suggested::new();
    for seed in &seeds.recordings {
        let found = asking::similar_recordings(pool, client, &seed.mbid, now).await?;
        let top = found
            .first()
            .map_or(1.0, |r| r.score)
            .max(f64::MIN_POSITIVE);
        for r in found.into_iter().take(PER_SEED_RECORDINGS) {
            let score = seed.weight * (r.score / top).clamp(0.0, 1.0);
            suggest(&mut best, &mine, score, r, &seed.because);
        }
    }
    Ok(best)
}

/// Adds to `best` the most listened-to recordings of the [`TOP_ARTISTS`] closest `artists`
/// alongside: each scored by its artist's score times its listens against their most
/// listened-to, with its artist's seed. An artist ListenBrainz refused them for is left out.
/// Then all of them, best first, ties broken on the recording.
async fn with_top_recordings(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    seeds: &seeds::Seeds,
    artists: &HashMap<String, (f64, Because)>,
    mut best: Suggested,
    now: i64,
) -> anyhow::Result<Vec<(f64, SimilarRecording, Because)>> {
    let mine: HashSet<&str> = seeds.recordings.iter().map(|s| s.mbid.as_str()).collect();
    let mut closest: Vec<(&String, &(f64, Because))> = artists.iter().collect();
    closest.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then_with(|| a.0.cmp(b.0)));
    for (artist, (score, because)) in closest.into_iter().take(TOP_ARTISTS) {
        let Some(found) = asking::top_recordings(pool, client, artist, now).await? else {
            continue;
        };
        let most = found
            .first()
            .map_or(1.0, |r| r.total_listen_count)
            .max(f64::MIN_POSITIVE);
        for r in found.into_iter().take(TOP_PER_ARTIST) {
            let score = score * (r.total_listen_count / most).clamp(0.0, 1.0);
            let r = SimilarRecording {
                recording_mbid: r.recording_mbid,
                recording_name: r.recording_name,
                artist_credit_name: r.artist_name,
                release_name: r.release_name,
                release_mbid: r.release_mbid,
                score: r.total_listen_count,
            };
            suggest(&mut best, &mine, score, r, because);
        }
    }
    let mut found: Vec<_> = best.into_values().collect();
    found.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.recording_mbid.cmp(&b.1.recording_mbid))
    });
    Ok(found)
}

/// The artists alongside the seed artists, each once with their best score and the seed that
/// gave them; none a seed artist themselves, whose music the listener knows.
async fn alike_artists(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    seeds: &seeds::Seeds,
    now: i64,
) -> anyhow::Result<HashMap<String, (f64, Because)>> {
    let mine: HashSet<&str> = seeds.artists.iter().map(|s| s.mbid.as_str()).collect();
    let mut best: HashMap<String, (f64, Because)> = HashMap::new();
    for seed in &seeds.artists {
        let found = asking::similar_artists(pool, client, &seed.mbid, now).await?;
        let top = found
            .first()
            .map_or(1.0, |a| a.score)
            .max(f64::MIN_POSITIVE);
        for a in found.into_iter().take(PER_SEED_ARTISTS) {
            if mine.contains(a.artist_mbid.as_str()) {
                continue;
            }
            let score = seed.weight * (a.score / top).clamp(0.0, 1.0);
            if best.get(&a.artist_mbid).is_none_or(|(was, _)| score > *was) {
                best.insert(a.artist_mbid, (score, seed.because.clone()));
            }
        }
    }
    Ok(best)
}

/// A library track's first artist (folded), its album, and its recording.
type Details = HashMap<i64, (String, Option<String>, Option<String>)>;

async fn details(pool: &SqlitePool, tracks: &[i64]) -> sqlx::Result<Details> {
    let rows: Vec<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT t.id,
             coalesce((SELECT ta.name FROM music_track_artists ta
                       WHERE ta.track_id = t.id AND ta.position = 0), t.artist, ''),
             coalesce(a.match_release_group_id, a.mb_release_group_id, 'album:' || t.album_id),
             coalesce(t.mb_recording_id, t.match_recording_id)
         FROM music_tracks t LEFT JOIN music_albums a ON a.id = t.album_id
         WHERE t.id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(tracks).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, artist, release, recording)| (id, (fold(&artist), release, recording)))
        .collect())
}

/// A song to request, from what ListenBrainz said of it.
fn song(r: SimilarRecording, meta: Option<&Recording>) -> Song {
    Song {
        mb_recording_id: r.recording_mbid,
        title: r.recording_name,
        artist: r.artist_credit_name,
        mb_artist_ids: meta.map(|m| m.artist_mbids.clone()).unwrap_or_default(),
        release: meta.and_then(|m| m.release.clone()).or(r.release_name),
        release_group: meta.and_then(|m| m.release_group_mbid.clone()),
    }
}

/// `user`'s list, made now from their seeds and what ListenBrainz says goes with them.
pub(super) async fn pick(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    user: i64,
    access: &Access,
    now: i64,
) -> anyhow::Result<Vec<Item>> {
    let flagged = track_ids(crate::aoide::listening::not_interested(pool, user).await?);
    let seeds = seeds::read(pool, user, access, &flagged, now).await?;
    let left_out = left_out(pool, user, &flagged, now).await?;
    let alike = alike_recordings(pool, client, &seeds, now).await?;
    let artists = alike_artists(pool, client, &seeds, now).await?;
    let alike = with_top_recordings(pool, client, &seeds, &artists, alike, now).await?;

    let ids: Vec<String> = alike
        .iter()
        .map(|(_, r, _)| r.recording_mbid.clone())
        .collect();
    let mut owned = by_recording(pool, access, &ids).await?;
    let unowned: Vec<String> = ids
        .into_iter()
        .filter(|id| !owned.contains_key(id))
        .collect();
    let meta = asking::recordings(pool, client, &unowned, now).await?;
    // Owned too: a track by one of its artists with the same title.
    let credited: HashSet<&str> = meta
        .values()
        .flat_map(|m| m.artist_mbids.iter().map(String::as_str))
        .collect();
    let mut titled: HashMap<(String, String), i64> = HashMap::new();
    for (id, title, artist) in by_artists(pool, access, &credited).await? {
        titled.entry((artist, fold(&title))).or_insert(id);
    }
    for (_, r, _) in &alike {
        let title = fold(&r.recording_name);
        let same = meta.get(&r.recording_mbid).and_then(|m| {
            m.artist_mbids
                .iter()
                .find_map(|a| titled.get(&(a.clone(), title.clone())))
        });
        if let Some(track) = same {
            owned.entry(r.recording_mbid.clone()).or_insert(*track);
        }
    }

    let mut playable: HashMap<i64, (f64, Because)> = HashMap::new();
    let mut offer = |track: i64, score: f64, because: &Because| {
        if !left_out.contains(&track) && playable.get(&track).is_none_or(|(was, _)| score > *was) {
            playable.insert(track, (score, because.clone()));
        }
    };
    let mut requestable = Vec::new();
    for (score, r, because) in alike {
        match owned.get(&r.recording_mbid) {
            Some(track) => offer(*track, score, &because),
            None => requestable.push((score, r, because)),
        }
    }
    let alongside: HashSet<&str> = artists.keys().map(String::as_str).collect();
    let mut per_artist: HashMap<String, usize> = HashMap::new();
    for (track, _, artist) in by_artists(pool, access, &alongside).await? {
        let considered = per_artist.entry(artist.clone()).or_default();
        if left_out.contains(&track) || *considered >= TRACKS_PER_ARTIST {
            continue;
        }
        *considered += 1;
        if let Some((score, because)) = artists.get(&artist) {
            offer(track, *score, because);
        }
    }
    candidates(pool, &seeds, playable, requestable, &meta)
        .await
        .map(rank)
}

/// The candidates to rank: tracks to play, but none a seed's own recording, and songs to
/// request, each song once by its artist and title.
async fn candidates(
    pool: &SqlitePool,
    seeds: &seeds::Seeds,
    playable: HashMap<i64, (f64, Because)>,
    requestable: Vec<(f64, SimilarRecording, Because)>,
    meta: &HashMap<String, Recording>,
) -> anyhow::Result<Vec<Candidate>> {
    let mine: HashSet<&str> = seeds.recordings.iter().map(|s| s.mbid.as_str()).collect();
    let tracks: Vec<i64> = playable.keys().copied().collect();
    let details = details(pool, &tracks).await?;
    let mut out = Vec::new();
    for (track, (score, because)) in playable {
        let Some((artist, release, recording)) = details.get(&track) else {
            continue;
        };
        if recording.as_deref().is_some_and(|r| mine.contains(r)) {
            continue;
        }
        out.push(Candidate {
            item: Item {
                track_id: Some(track),
                song: None,
                because,
            },
            score,
            order: format!("track:{track:012}"),
            artist: artist.clone(),
            release: release.clone(),
        });
    }
    let mut songs = HashSet::new();
    for (score, r, because) in requestable {
        let found = meta.get(&r.recording_mbid);
        let artist = fold(
            found
                .and_then(|m| m.artist.as_deref())
                .unwrap_or(&r.artist_credit_name),
        );
        if !songs.insert((artist.clone(), fold(&r.recording_name))) {
            continue;
        }
        let song = song(r, found);
        out.push(Candidate {
            order: format!("song:{}", song.mb_recording_id),
            release: song.release_group.clone(),
            item: Item {
                track_id: None,
                song: Some(song),
                because,
            },
            score,
            artist,
        });
    }
    Ok(out)
}

/// The kept list's songs still to offer, in order: those to play still present for `access`
/// and not heard lately, finished often or flagged by `user` since; those to request still not
/// in the library for them.
pub(super) async fn still_there(
    pool: &SqlitePool,
    user: i64,
    access: &Access,
    items: Vec<Item>,
    now: i64,
) -> anyhow::Result<Vec<Item>> {
    let flagged = track_ids(crate::aoide::listening::not_interested(pool, user).await?);
    let left_out = left_out(pool, user, &flagged, now).await?;
    let tracks: Vec<i64> = items.iter().filter_map(|i| i.track_id).collect();
    let present: HashSet<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT t.id FROM music_tracks t WHERE t.missing_since IS NULL AND {}
           AND t.id IN (SELECT value FROM json_each(?))",
        access.track_sql("t")
    )))
    .bind(serde_json::to_string(&tracks)?)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let songs: Vec<String> = items
        .iter()
        .filter_map(|i| Some(i.song.as_ref()?.mb_recording_id.clone()))
        .collect();
    let arrived = by_recording(pool, access, &songs).await?;
    Ok(items
        .into_iter()
        .filter(|i| match (&i.track_id, &i.song) {
            (Some(track), _) => present.contains(track) && !left_out.contains(track),
            (None, Some(song)) => !arrived.contains_key(&song.mb_recording_id),
            (None, None) => false,
        })
        .collect())
}
