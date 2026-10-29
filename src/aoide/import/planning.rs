//! Reading the sidecar's database into a plan: its users, ops, tracks and playlist artwork.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct OpRow {
    pub(super) seq: i64,
    pub(super) op_id: String,
    pub(super) user_id: String,
    pub(super) device_id: String,
    pub(super) entity: String,
    pub(super) entity_id: String,
    pub(super) operation: String,
    pub(super) payload: String,
    pub(super) created_at: i64,
    pub(super) received_at: i64,
    pub(super) playlist_id: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(super) struct ImageRow {
    pub(super) user_id: String,
    pub(super) image_hash: String,
    pub(super) mime_type: String,
    pub(super) bytes: Vec<u8>,
    pub(super) created_at: i64,
}

/// What a sidecar user becomes in Melete.
#[derive(Debug, PartialEq)]
pub enum Mapping {
    /// Mapped to this Melete user: their id and username.
    To(i64, String),
    /// Left out with `--skip-user`.
    Skipped,
    /// Neither mapped nor skipped: the import refuses to apply until it is.
    Unmapped,
}

/// A user in the sidecar's data and what they map to.
pub struct SidecarUser {
    /// Their Jellyfin user id, without dashes and in lowercase.
    pub jellyfin_id: String,
    /// How many ops they have in the sidecar.
    pub ops: i64,
    /// The device names their queue states carry, for telling who's who.
    pub device_names: Vec<String>,
    /// The Melete user they map to, or that they're skipped or not mapped yet.
    pub mapping: Mapping,
}

/// Track references, counted per record.
#[derive(Default)]
pub struct Tracks {
    /// References found by an exact content key.
    pub exact: usize,
    /// References found with the duration a second either side.
    pub near: usize,
    /// Content key to how many records carry it.
    pub ambiguous: BTreeMap<String, usize>,
    /// Content keys that fit no track, to how many records carry each.
    pub missing: BTreeMap<String, usize>,
    /// Records naming a track with no content key to find it by.
    pub keyless: usize,
}

/// What the import would write, read from the sidecar's copy and Melete's library.
pub struct Plan {
    /// Every user in the sidecar's data, mapped or not.
    pub users: Vec<SidecarUser>,
    pub(super) ops: Vec<(OpRow, i64)>,
    /// Ops that would be written, by entity.
    pub by_entity: BTreeMap<String, usize>,
    /// Queue states left behind.
    pub queue_states: usize,
    /// How the track references were found.
    pub tracks: Tracks,
    /// Playlists' Jellyfin artwork and source ids made null.
    pub cleared: usize,
    pub(super) owners: Vec<(String, i64, i64)>,
    pub(super) shares: Vec<(String, i64, i64, i64)>,
    pub(super) images: Vec<(i64, ImageRow)>,
    /// Image hashes whose bytes don't hash to them, or of a type Melete doesn't keep.
    pub bad_images: Vec<String>,
}

/// Reads the sidecar's copy, maps users and tracks, and says what would be written.
pub async fn plan(
    sidecar: &SqlitePool,
    melete: &SqlitePool,
    request: &Request,
) -> anyhow::Result<Plan> {
    let users = sidecar_users(sidecar, melete, request).await?;
    let melete_id = |jellyfin: &str| {
        users.iter().find_map(|u| match &u.mapping {
            Mapping::To(id, _) if u.jellyfin_id == jellyfin => Some(*id),
            _ => None,
        })
    };

    let library = Library::load(melete).await?;
    let rows: Vec<OpRow> = sqlx::query_as(
        "SELECT seq, op_id, user_id, device_id, entity, entity_id, operation, payload, created_at,
             received_at, playlist_id
         FROM ops ORDER BY seq",
    )
    .fetch_all(sidecar)
    .await?;
    let mut ops = Vec::with_capacity(rows.len());
    let mut by_entity = BTreeMap::new();
    let mut queue_states = 0;
    let mut tracks = Tracks::default();
    let mut cleared = 0;
    for mut row in rows {
        if row.entity == "queue_state" {
            queue_states += 1;
            continue;
        }
        let Some(user) = melete_id(&row.user_id) else {
            continue;
        };
        if !ENTITIES.contains(&row.entity.as_str()) {
            bail!("op {} has the unknown entity {:?}", row.seq, row.entity);
        }
        if TRACK_ENTITIES.contains(&row.entity.as_str()) {
            map_track(&mut row, &library, &mut tracks)?;
        }
        // A playlist's chosen artwork and its source playlist were Jellyfin items, which
        // name nothing here; the apps would ask Melete for their pictures and get none.
        if row.entity == "playlists" {
            cleared += clear_jellyfin_ids(&mut row)?;
        }
        *by_entity.entry(row.entity.clone()).or_default() += 1;
        ops.push((row, user));
    }

    let (owners, shares) = owners_and_shares(sidecar, &melete_id).await?;
    let (images, bad_images) = images(sidecar, &melete_id).await?;
    Ok(Plan {
        users,
        ops,
        by_entity,
        queue_states,
        tracks,
        cleared,
        owners,
        shares,
        images,
        bad_images,
    })
}

/// The sidecar's users, with how many ops each has, their devices' names, and who each is in
/// Melete, refusing a user asked about that the sidecar doesn't have.
async fn sidecar_users(
    sidecar: &SqlitePool,
    melete: &SqlitePool,
    request: &Request,
) -> anyhow::Result<Vec<SidecarUser>> {
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT user_id, sum(n) FROM (
             SELECT user_id, count(*) AS n FROM ops GROUP BY user_id
             UNION ALL SELECT owner_user_id, 0 FROM playlist_owners
             UNION ALL SELECT grantee_user_id, 0 FROM playlist_shares
             UNION ALL SELECT user_id, 0 FROM playlist_images)
         GROUP BY user_id ORDER BY user_id",
    )
    .fetch_all(sidecar)
    .await
    .context("reading the sidecar's users (is this aoide-sync.db?)")?;
    let names: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT user_id, coalesce(json_extract(payload, '$.deviceName'),
                                           json_extract(payload, '$.device_name'))
         FROM ops WHERE entity = 'queue_state' AND json_valid(payload)
           AND coalesce(json_extract(payload, '$.deviceName'), json_extract(payload, '$.device_name'))
               IS NOT NULL
         ORDER BY 1, 2",
    )
    .fetch_all(sidecar)
    .await?;
    for id in request.users.keys().chain(&request.skipped) {
        if !counts.iter().any(|(user, _)| user == id) {
            bail!("no user {id} in the sidecar's data");
        }
    }
    let mut users = Vec::with_capacity(counts.len());
    for (id, ops) in counts {
        let mapping = if request.skipped.contains(&id) {
            Mapping::Skipped
        } else if let Some(name) = request.users.get(&id) {
            let user = crate::auth::find_user(melete, name)
                .await?
                .with_context(|| format!("no Melete user named {name:?}"))?;
            Mapping::To(user.id, user.username)
        } else {
            Mapping::Unmapped
        };
        let device_names = names
            .iter()
            .filter(|(user, _)| *user == id)
            .map(|(_, name)| name.clone())
            .collect();
        users.push(SidecarUser {
            jellyfin_id: id,
            ops,
            device_names,
            mapping,
        });
    }
    Ok(users)
}

/// Points an op's track at the library's copy of it, when one is found by its content key,
/// counting how each reference was found.
fn map_track(row: &mut OpRow, library: &Library, tracks: &mut Tracks) -> anyhow::Result<()> {
    let payload: Value = serde_json::from_str(&row.payload)
        .with_context(|| format!("op {} has a payload that isn't JSON", row.seq))?;
    if let Some((key, old)) = field(&payload, ["jellyfinId", "jellyfin_id"]) {
        let found = match field(&payload, ["contentKey", "content_key"]) {
            None => {
                tracks.keyless += 1;
                None
            }
            Some((_, content_key)) => match library.find(content_key) {
                Found::Exact(id) => {
                    tracks.exact += 1;
                    Some(id)
                }
                Found::Near(id) => {
                    tracks.near += 1;
                    Some(id)
                }
                Found::Ambiguous => {
                    *tracks.ambiguous.entry(content_key.to_owned()).or_default() += 1;
                    None
                }
                Found::Missing => {
                    *tracks.missing.entry(content_key.to_owned()).or_default() += 1;
                    None
                }
            },
        };
        if let Some(id) = found {
            row.payload = rewrite(&row.payload, key, old, &id.to_string())
                .with_context(|| format!("couldn't rewrite the track id in op {}", row.seq))?;
        }
    }
    Ok(())
}

/// Clears a playlist op's Jellyfin artwork and source playlist ids, saying how many it cleared.
fn clear_jellyfin_ids(row: &mut OpRow) -> anyhow::Result<usize> {
    let mut cleared = 0;
    let payload: Value = serde_json::from_str(&row.payload)
        .with_context(|| format!("op {} has a payload that isn't JSON", row.seq))?;
    for names in JELLYFIN_PLAYLIST_IDS {
        if let Some((key, old)) = field(&payload, names) {
            row.payload = clear(&row.payload, key, old)
                .with_context(|| format!("couldn't clear {key} in op {}", row.seq))?;
            cleared += 1;
        }
    }
    Ok(cleared)
}

/// Playlists' owners, and their shares, by Melete user.
async fn owners_and_shares(
    sidecar: &SqlitePool,
    melete_id: &impl Fn(&str) -> Option<i64>,
) -> anyhow::Result<(Vec<(String, i64, i64)>, Vec<(String, i64, i64, i64)>)> {
    let owner_rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT playlist_id, owner_user_id, created_at FROM playlist_owners")
            .fetch_all(sidecar)
            .await?;
    let owners: Vec<(String, i64, i64)> = owner_rows
        .into_iter()
        .filter_map(|(playlist, owner, at)| Some((playlist, melete_id(&owner)?, at)))
        .collect();
    let share_rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT playlist_id, grantee_user_id, can_edit, created_at FROM playlist_shares",
    )
    .fetch_all(sidecar)
    .await?;
    // A share of a playlist whose owner stays behind would share nothing.
    let shares = share_rows
        .into_iter()
        .filter(|(playlist, ..)| owners.iter().any(|(p, ..)| p == playlist))
        .filter_map(|(playlist, grantee, edit, at)| {
            Some((playlist, melete_id(&grantee)?, edit, at))
        })
        .collect();
    Ok((owners, shares))
}

/// Playlists' artwork by Melete user, apart from images whose bytes don't hash to their name
/// or whose type Melete doesn't keep.
async fn images(
    sidecar: &SqlitePool,
    melete_id: &impl Fn(&str) -> Option<i64>,
) -> anyhow::Result<(Vec<(i64, ImageRow)>, Vec<String>)> {
    let image_rows: Vec<ImageRow> = sqlx::query_as(
        "SELECT user_id, image_hash, mime_type, bytes, created_at FROM playlist_images",
    )
    .fetch_all(sidecar)
    .await?;
    let mut images = Vec::new();
    let mut bad_images = Vec::new();
    for image in image_rows {
        let Some(user) = melete_id(&image.user_id) else {
            continue;
        };
        if is_hash(&image.image_hash)
            && sha256_hex(&image.bytes) == image.image_hash
            && IMAGE_TYPES.contains(&image.mime_type.as_str())
        {
            images.push((user, image));
        } else {
            bad_images.push(image.image_hash);
        }
    }
    Ok((images, bad_images))
}
