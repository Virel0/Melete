//! `melete import aoide`: the Jellyfin aoide-sidecar's sync data, read once from a copy of its
//! database (`aoide-sync.db` with its `-wal` and `-shm`) and written into Melete's own sync
//! store.
//!
//! What comes over: every op (with its sequence number, so the cursors phones hold stay
//! valid), playlist owners and shares, and playlist artwork. What doesn't: queue states (bare
//! Jellyfin ids, and only a device's latest matters), device cursors (Melete counts each
//! session as a device), export bookkeeping (the export into Jellyfin's playlists is dropped,
//! as there's no Jellyfin) and measurements (recomputed from the audio).
//!
//! Users are mapped by hand, Jellyfin user id to Melete name, and every one in the file must be
//! mapped or skipped. Tracks are found by the `contentKey` each record carries, the apps' own
//! key for relinking a track whose id changed (`artist|album|title|seconds`); a key that fits
//! no track, or more than one, is listed and its record left as it was, never guessed.
//! Without `--apply` nothing is written.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, bail};
use serde_json::Value;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use super::library::{is_hash, sha256_hex};
use super::sync::ENTITIES;

mod applying;
mod matching;
mod planning;
mod reporting;
mod rewrite;

pub use applying::*;
use matching::*;
pub use planning::*;
pub use reporting::*;
use rewrite::*;

/// Entities whose records name a track, by `jellyfinId` and `contentKey`.
const TRACK_ENTITIES: [&str; 4] = ["play_events", "playlist_items", "likes", "track_flags"];
const IMAGE_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];
/// A playlist's Jellyfin ids with no track behind them: its chosen artwork and its source.
const JELLYFIN_PLAYLIST_IDS: [[&str; 2]; 2] = [
    ["artworkItemId", "artwork_item_id"],
    ["sourceJellyfinId", "source_jellyfin_id"],
];

/// How to run the import, shown when its arguments don't parse.
pub const USAGE: &str = "melete import aoide <aoide-sync.db> [--user <jellyfin-id>=<name>]... [--skip-user <jellyfin-id>]... [--apply]";

/// What `melete import aoide` was asked to do.
#[derive(Debug, Default)]
pub struct Request {
    /// The path to the copy of `aoide-sync.db`.
    pub path: String,
    /// Jellyfin user id (32 lowercase hex digits) to Melete username.
    pub users: HashMap<String, String>,
    /// Jellyfin user ids (32 lowercase hex digits) whose data is left out.
    pub skipped: HashSet<String>,
    /// Whether to write; without it the import only reports what it would do.
    pub apply: bool,
}

/// Jellyfin shows user ids with and without dashes, in either case.
fn jellyfin_id(text: &str) -> String {
    text.replace('-', "").to_ascii_lowercase()
}

/// Reads `melete import aoide`'s arguments (after `aoide`).
///
/// # Errors
///
/// Fails, with the usage, when the path is missing or a flag is unknown; and when `--user` or
/// `--skip-user` has no value, or `--user`'s value has no `=`.
pub fn parse(args: &[&str]) -> anyhow::Result<Request> {
    let [path, flags @ ..] = args else {
        bail!("{USAGE}");
    };
    let mut request = Request {
        path: (*path).to_owned(),
        ..Request::default()
    };
    let mut flags = flags.iter();
    while let Some(flag) = flags.next() {
        match *flag {
            "--apply" => request.apply = true,
            "--user" => {
                let pair = flags.next().context("--user needs <jellyfin-id>=<name>")?;
                let (id, name) = pair
                    .split_once('=')
                    .context("--user needs <jellyfin-id>=<name>")?;
                request.users.insert(jellyfin_id(id), name.to_owned());
            }
            "--skip-user" => {
                let id = flags
                    .next()
                    .context("--skip-user needs a Jellyfin user id")?;
                request.skipped.insert(jellyfin_id(id));
            }
            other => bail!("unknown option {other:?}\n{USAGE}"),
        }
    }
    Ok(request)
}

/// The phone's folding, in its order: accents off, lowercase, trimmed, then one pass
/// collapsing double spaces.
fn normalise(text: &str) -> String {
    let folded: String = text
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded.trim().replace("  ", " ")
}

fn present(text: &Option<String>) -> Option<&str> {
    text.as_deref().filter(|s| !s.trim().is_empty())
}

/// The first of `names` that's a string in the payload, with its value.
fn field<'a>(payload: &'a Value, names: [&'static str; 2]) -> Option<(&'static str, &'a str)> {
    names
        .into_iter()
        .find_map(|name| payload[name].as_str().map(|value| (name, value)))
}

/// `melete import aoide …`, inside the container.
pub async fn run(melete: &SqlitePool, args: &[&str]) -> anyhow::Result<()> {
    let request = parse(args)?;
    if !std::path::Path::new(&request.path).is_file() {
        bail!("no file at {}", request.path);
    }
    let options = SqliteConnectOptions::new()
        .filename(&request.path)
        .read_only(true)
        // The container's root filesystem is read-only, so SQLite has no temp folder.
        .pragma("temp_store", "memory");
    let sidecar = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("opening {}", request.path))?;
    let plan = plan(&sidecar, melete, &request).await?;
    print!("{}", report(&plan));
    if request.apply {
        apply(melete, &plan).await?;
        println!("Imported.");
    } else {
        println!("Nothing written. Run again with --apply to import.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const U1: &str = "a1b2c3d4a1b2c3d4a1b2c3d4a1b2c3d4";
    const U2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const U3: &str = "cccccccccccccccccccccccccccccccc";

    /// The sidecar's own tables (SyncDatabase.cs, schema v4 onwards), with one of everything.
    async fn sidecar(dir: &std::path::Path, image: &[u8]) -> (SqlitePool, Vec<(i64, String)>) {
        let options = SqliteConnectOptions::new()
            .filename(dir.join("aoide-sync.db"))
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE ops (seq INTEGER PRIMARY KEY AUTOINCREMENT, op_id TEXT NOT NULL,
                 user_id TEXT NOT NULL, device_id TEXT NOT NULL, entity TEXT NOT NULL,
                 entity_id TEXT NOT NULL, operation TEXT NOT NULL, payload TEXT NOT NULL,
                 created_at INTEGER NOT NULL, received_at INTEGER NOT NULL, playlist_id TEXT);
             CREATE UNIQUE INDEX idx_ops_user_opid ON ops (user_id, op_id);
             CREATE TABLE exported_playlists (user_id TEXT NOT NULL, aoide_playlist_id TEXT NOT NULL,
                 jellyfin_item_id TEXT NOT NULL, content_hash TEXT NOT NULL,
                 exported_at INTEGER NOT NULL, PRIMARY KEY (user_id, aoide_playlist_id));
             CREATE TABLE playlist_images (user_id TEXT NOT NULL, image_hash TEXT NOT NULL,
                 mime_type TEXT NOT NULL, bytes BLOB NOT NULL, size INTEGER NOT NULL,
                 created_at INTEGER NOT NULL, PRIMARY KEY (user_id, image_hash));
             CREATE TABLE device_cursors (user_id TEXT NOT NULL, device_id TEXT NOT NULL,
                 cursor INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (user_id, device_id));
             CREATE TABLE playlist_owners (playlist_id TEXT PRIMARY KEY, owner_user_id TEXT NOT NULL,
                 created_at INTEGER NOT NULL);
             CREATE TABLE playlist_shares (playlist_id TEXT NOT NULL, grantee_user_id TEXT NOT NULL,
                 can_edit INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL,
                 PRIMARY KEY (playlist_id, grantee_user_id));",
        )
        .execute(&pool)
        .await
        .unwrap();
        let hash = sha256_hex(image);
        let ops: Vec<(i64, &str, &str, &str, String)> = vec![
            (10, U1, "playlists", "pl1", format!(r#"{{"id":"pl1","name":"Mix","imageHash":"{hash}"}}"#)),
            (11, U1, "playlist_items", "i1",
             r#"{"id":"i1","playlistId":"pl1","jellyfinId": "11111111111111111111111111111111","contentKey":"bjork|post|army of me|234","position":"a0","updatedAt":1792874482251,"gain":0.10}"#.into()),
            (12, U1, "play_events", "e1",
             r#"{"id":"e1","jellyfin_id":"22222222222222222222222222222222","content_key":"bjork|homogenic|joga|306","msPlayed":210000}"#.into()),
            (13, U1, "likes", "l1",
             r#"{"id":"l1","jellyfinId":"33333333333333333333333333333333","contentKey":"a|hits|song|200","liked":true}"#.into()),
            (14, U1, "track_flags", "f1",
             r#"{"id":"f1","jellyfinId":"44444444444444444444444444444444","contentKey":"nobody|nothing|none|100","notInterested":true}"#.into()),
            (15, U1, "queue_state", "rig",
             r#"{"deviceId":"rig","deviceName":"Living room iPad","trackIds":"[\"1111\"]"}"#.into()),
            (16, U2, "likes", "l2",
             r#"{"id":"l2","jellyfinId":"11111111111111111111111111111111","contentKey":"bjork|post|army of me|234","liked":true}"#.into()),
            (17, U3, "play_events", "e2",
             r#"{"id":"e2","jellyfinId":"11111111111111111111111111111111","contentKey":"bjork|post|army of me|234"}"#.into()),
            (18, U1, "likes", "l3",
             r#"{"id":"l3","jellyfinId":"55555555555555555555555555555555","contentKey":"massive attack|protection|protection|471","liked":true}"#.into()),
            (20, U1, "folders", "d1", r#"{"id":"d1","name":"Moods"}"#.into()),
        ];
        for (seq, user, entity, entity_id, payload) in &ops {
            let playlist = match *entity {
                "playlists" => Some(*entity_id),
                "playlist_items" => Some("pl1"),
                _ => None,
            };
            sqlx::query(
                "INSERT INTO ops (seq, op_id, user_id, device_id, entity, entity_id, operation,
                     payload, created_at, received_at, playlist_id)
                 VALUES (?, ?, ?, 'rig', ?, ?, 'upsert', ?, 1792874482251, 1792874482299, ?)",
            )
            .bind(seq)
            .bind(format!("op{seq}"))
            .bind(user)
            .bind(entity)
            .bind(entity_id)
            .bind(payload)
            .bind(playlist)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO playlist_owners VALUES ('pl1', ?1, 1), ('pl2', ?3, 2);
             INSERT INTO playlist_shares VALUES ('pl1', ?2, 0, 3), ('pl2', ?1, 1, 4);
             INSERT INTO playlist_images VALUES (?1, ?4, 'image/png', ?5, 4, 5),
                 (?1, 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff', 'image/png', ?5, 4, 6);
             INSERT INTO exported_playlists VALUES (?1, 'pl1', 'jf', 'h', 7);
             INSERT INTO device_cursors VALUES (?1, 'rig', 20, 8);",
        )
        .bind(U1)
        .bind(U2)
        .bind(U3)
        .bind(&hash)
        .bind(image)
        .execute(&pool)
        .await
        .unwrap();
        let payloads = ops.into_iter().map(|op| (op.0, op.4)).collect();
        (pool, payloads)
    }

    async fn melete() -> SqlitePool {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'Post', 'Björk', 'a', 1), (2, 1, 'k2', 'Homogenic', 'Björk', 'b', 1),
                        (3, 1, 'k3', 'Hits', 'Various Artists', 'c', 1),
                        (4, 1, 'k4', 'Protection', 'Massive Attack', 'd', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, album,
                 duration_seconds, missing_since) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Army of Me', 'Björk', 'Post', 234.4, NULL),
                 (2, 1, 'b/1', 1, 1, 1, 1, 2, 1, 'Jóga', NULL, 'Homogenic', 305.4, NULL),
                 (3, 1, 'c/1', 1, 1, 1, 1, 3, 1, 'Song', 'A', 'Hits', 200.0, NULL),
                 (4, 1, 'c/2', 1, 1, 1, 1, 3, 1, 'Song', 'A', 'Hits', 200.2, NULL),
                 (5, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Nothing', 'Nobody', 'Nothing', 100.0, 9),
                 (6, 1, 'd/1', 1, 1, 1, 1, 4, 1, 'Protection', 'Massive Attack; Tracey Thorn',
                  'Protection', 471.0, NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
        for name in ["alice", "bob"] {
            crate::auth::create_user(&pool, name, "a-password".into(), false)
                .await
                .unwrap()
                .unwrap();
        }
        pool
    }

    fn request(args: &[&str]) -> Request {
        let mut all = vec!["aoide-sync.db"];
        all.extend(args);
        parse(&all).unwrap()
    }

    #[tokio::test]
    async fn imports_ops_with_their_numbers_and_tracks_found_by_content_key() {
        let dir = tempfile::tempdir().unwrap();
        let (sidecar, payloads) = sidecar(dir.path(), b"\x89PNG").await;
        let melete = melete().await;

        // Every user must be accounted for before anything is written.
        let partial = plan(
            &sidecar,
            &melete,
            &request(&["--user", &format!("{U1}=alice")]),
        )
        .await
        .unwrap();
        assert!(report(&partial).contains("NOT MAPPED"));
        let refused = apply(&melete, &partial).await.unwrap_err().to_string();
        assert!(refused.contains(U2), "{refused}");
        let wrong_name = format!("{U1}=carol");
        assert!(
            plan(&sidecar, &melete, &request(&["--user", &wrong_name]))
                .await
                .is_err()
        );

        let bob = format!("{}={}", U2.to_uppercase(), "bob");
        let full = request(&[
            "--user",
            &format!("{U1}=alice"),
            "--user",
            &bob,
            "--skip-user",
            U3,
        ]);
        let planned = plan(&sidecar, &melete, &full).await.unwrap();
        let text = report(&planned);
        assert!(text.contains("Living room iPad"), "{text}");
        assert!(text.contains("nobody|nothing|none|100"), "{text}");
        assert_eq!((planned.tracks.exact, planned.tracks.near), (3, 1));
        assert_eq!(planned.tracks.ambiguous.get("a|hits|song|200"), Some(&1));
        assert_eq!(planned.bad_images.len(), 1);
        apply(&melete, &planned).await.unwrap();

        let rows: Vec<(i64, i64, String)> =
            sqlx::query_as("SELECT seq, user_id, payload FROM aoide_ops ORDER BY seq")
                .fetch_all(&melete)
                .await
                .unwrap();
        let seqs: Vec<i64> = rows.iter().map(|r| r.0).collect();
        assert_eq!(
            seqs,
            [10, 11, 12, 13, 14, 16, 18, 20],
            "queue states and skipped users stay"
        );
        let original = |seq: i64| payloads.iter().find(|p| p.0 == seq).unwrap().1.clone();
        let stored = |seq: i64| rows.iter().find(|r| r.0 == seq).unwrap().2.clone();
        assert_eq!(
            stored(11),
            original(11).replace("11111111111111111111111111111111", "1")
        );
        assert_eq!(
            stored(12),
            original(12).replace("22222222222222222222222222222222", "2")
        );
        assert_eq!(
            stored(18),
            original(18).replace("55555555555555555555555555555555", "6")
        );
        for unchanged in [10, 13, 14, 20] {
            assert_eq!(stored(unchanged), original(unchanged));
        }
        assert_eq!(rows.iter().find(|r| r.0 == 16).unwrap().1, 2, "bob's");
        let next: i64 =
            sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name = 'aoide_ops'")
                .fetch_one(&melete)
                .await
                .unwrap();
        assert_eq!(next, 20, "new ops follow on");
        let owners: Vec<(String, i64)> =
            sqlx::query_as("SELECT playlist_id, owner_user_id FROM aoide_playlist_owners")
                .fetch_all(&melete)
                .await
                .unwrap();
        assert_eq!(owners, [("pl1".to_owned(), 1)]);
        let shares: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT playlist_id, grantee_user_id, can_edit FROM aoide_playlist_shares",
        )
        .fetch_all(&melete)
        .await
        .unwrap();
        assert_eq!(shares, [("pl1".to_owned(), 2, 0)]);
        let images: i64 = sqlx::query_scalar("SELECT count(*) FROM aoide_images")
            .fetch_one(&melete)
            .await
            .unwrap();
        assert_eq!(images, 1);
        let cursors: i64 = sqlx::query_scalar("SELECT count(*) FROM aoide_device_cursors")
            .fetch_one(&melete)
            .await
            .unwrap();
        assert_eq!(cursors, 0);

        let again = apply(&melete, &planned).await.unwrap_err().to_string();
        assert!(again.contains("already been used"), "{again}");
    }

    #[test]
    fn rewrites_only_the_one_value() {
        let raw = r#"{"a":1.10, "jellyfinId" : "ab", "n":12345678901234567890}"#;
        assert_eq!(
            rewrite(raw, "jellyfinId", "ab", "7").as_deref(),
            Some(r#"{"a":1.10, "jellyfinId" : "7", "n":12345678901234567890}"#)
        );
        assert_eq!(
            rewrite(
                r#"{"jellyfinId":"ab","x":{"jellyfinId":"ab"}}"#,
                "jellyfinId",
                "ab",
                "7"
            ),
            None
        );
        // The key quoted inside a string has escaped quotes, so it isn't mistaken for the key.
        assert_eq!(
            rewrite(
                r#"{"note":"\"jellyfinId\":\"cd\"","jellyfinId":"cd"}"#,
                "jellyfinId",
                "cd",
                "7"
            )
            .as_deref(),
            Some(r#"{"note":"\"jellyfinId\":\"cd\"","jellyfinId":"7"}"#)
        );
        // Found only nested: the check against the parsed payload refuses it.
        assert_eq!(
            rewrite(r#"{"x":{"jellyfinId":"ab"}}"#, "jellyfinId", "ab", "7"),
            None
        );
    }

    #[test]
    fn folds_keys_as_the_phone_does() {
        assert_eq!(normalise("Björk"), normalise("  bjork "));
        assert_eq!(normalise("ARMY  OF ME"), "army of me");
        let parsed = request(&[
            "--user",
            "A1B2C3D4-A1B2-C3D4-A1B2-C3D4A1B2C3D4=alice",
            "--apply",
        ]);
        assert_eq!(parsed.users.get(U1).map(String::as_str), Some("alice"));
        assert!(parsed.apply);
        assert!(parse(&["db", "--force"]).is_err());
    }

    #[tokio::test]
    async fn a_playlists_jellyfin_artwork_and_source_are_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let (sidecar, _) = sidecar(dir.path(), b"img").await;
        let payload = r#"{"id":"pl3","name":"Old mix","artworkItemId":"a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1","sourceJellyfinId" : "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2","gain":0.10}"#;
        sqlx::query(
            "INSERT INTO ops (seq, op_id, user_id, device_id, entity, entity_id, operation,
                 payload, created_at, received_at, playlist_id)
             VALUES (30, 'op30', ?, 'rig', 'playlists', 'pl3', 'upsert', ?, 1, 1, 'pl3')",
        )
        .bind(U1)
        .bind(payload)
        .execute(&sidecar)
        .await
        .unwrap();
        let melete = melete().await;
        let planned = plan(
            &sidecar,
            &melete,
            &request(&[
                "--user",
                &format!("{U1}=alice"),
                "--skip-user",
                U2,
                "--skip-user",
                U3,
            ]),
        )
        .await
        .unwrap();
        assert_eq!(planned.cleared, 2);
        assert!(
            report(&planned)
                .contains("Jellyfin artwork and source ids cleared (they name nothing here): 2")
        );
        apply(&melete, &planned).await.unwrap();
        let stored: String = sqlx::query_scalar("SELECT payload FROM aoide_ops WHERE seq = 30")
            .fetch_one(&melete)
            .await
            .unwrap();
        assert_eq!(
            stored,
            r#"{"id":"pl3","name":"Old mix","artworkItemId":null,"sourceJellyfinId" : null,"gain":0.10}"#,
            "everything else as it was written"
        );
    }

    #[test]
    fn clears_only_the_one_value_it_was_asked_to() {
        assert_eq!(
            clear(r#"{"a":"x","b":1.50}"#, "a", "x").as_deref(),
            Some(r#"{"a":null,"b":1.50}"#)
        );
        assert_eq!(clear(r#"{"a":"y"}"#, "a", "x"), None);
        assert_eq!(
            clear(r#"{"a":"x","n":{"a":"x"}}"#, "a", "x"),
            None,
            "named twice"
        );
    }
}
