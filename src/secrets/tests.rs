use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use sqlx::SqlitePool;

use super::recovery::clear;
use super::startup::{Unreadables, prepare, switch, unreadable};
use super::*;
use crate::{auth, db, settings};

/// The plain values the tests store: none may be found where secrets are kept.
const TOKEN: &str = "plain-tmdb-token-value";
const INDEXER: &str = "plain-indexer-key-value";
const TWO_FACTOR: [u8; 20] = *b"twenty-byte-secret!!";
const LINK: &str = "http://192.168.1.10:9696/1/download?apikey=plain-link-key";
const GUID: &str = "https://tracker.example/t/1?passkey=plain-passkey";

fn other_key() -> Key {
    Key::from_bytes(&[9; KEY_LEN])
}

/// A database in `dir` as one from before sealing: a secret setting, a plain one, an indexer
/// with a key and one without, a search result with its link, and a user with two-factor
/// sign-in.
async fn plain_database(dir: &Path) -> SqlitePool {
    let pool = db::open(dir).await.unwrap();
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TMDB_TOKEN', ?, 0),
             ('MELETE_LYRICS', 'lrclib', 0);
         INSERT INTO indexers (id, name, url, api_key, enabled, categories, movie_search_params,
             tv_search_params, source, source_id, added_at)
         VALUES (1, 'Keyed', 'http://192.168.1.10:9696/1/api', ?, 1, '[2000]', '[\"q\"]', '[]',
             'prowlarr', 1, 0),
             (2, 'Keyless', 'https://nyaa.si', '', 1, '[5000]', '[]', '[]', 'melete', NULL, 0);
         INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
         VALUES (1, 1, 'Some Movie', 1, 'radarr', 0);
         INSERT INTO searches (id, wanted_movie_id, searched_at, indexers) VALUES (1, 1, 0, '[]');
         INSERT INTO release_decisions (id, search_id, indexer_id, title, download_url, guid,
             accepted, decision)
         VALUES (1, 1, 1, 'Some.Movie.2024.1080p', ?, ?, 1, '{}')",
    )
    .bind(TOKEN)
    .bind(INDEXER)
    .bind(LINK)
    .bind(GUID)
    .execute(&pool)
    .await
    .unwrap();
    let alice = auth::create_user(&pool, "alice", auth::generate_password().unwrap(), false)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "INSERT INTO two_factor (user_id, secret, created_at, confirmed_at) VALUES (?, ?, 0, 0)",
    )
    .bind(alice.id)
    .bind(&TWO_FACTOR[..])
    .execute(&pool)
    .await
    .unwrap();
    pool
}

/// The three secrets as they're stored.
async fn raw(pool: &SqlitePool) -> (String, String, Vec<u8>) {
    sqlx::query_as(
        "SELECT (SELECT value FROM settings WHERE name = 'MELETE_TMDB_TOKEN'),
                (SELECT api_key FROM indexers WHERE id = 1),
                (SELECT secret FROM two_factor)",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The search result's link and guid as they're stored.
async fn links(pool: &SqlitePool) -> (String, String) {
    sqlx::query_as("SELECT download_url, guid FROM release_decisions WHERE id = 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[test]
fn a_sealed_value_opens_only_with_its_key_and_in_its_place() {
    let key = key().unwrap();
    let first = key.seal_text("settings.A", TOKEN).unwrap();
    let again = key.seal_text("settings.A", TOKEN).unwrap();
    assert!(first.starts_with("sealed:v1:") && !first.contains(TOKEN));
    assert_ne!(first, again, "a fresh nonce for every value");
    assert_eq!(key.open_text("settings.A", &first).unwrap(), TOKEN);
    assert_eq!(key.open_text("settings.A", &again).unwrap(), TOKEN);
    assert_eq!(
        other_key().open_text("settings.A", &first),
        Err(Unreadable::WrongKey)
    );
    assert_eq!(
        key.open_text("settings.B", &first),
        Err(Unreadable::WrongKey),
        "moved to another setting, it doesn't open"
    );
    let mut altered = first.clone().into_bytes();
    let last = altered.len() - 3;
    altered[last] = if altered[last] == b'A' { b'B' } else { b'A' };
    assert_eq!(
        key.open_text("settings.A", &String::from_utf8(altered).unwrap()),
        Err(Unreadable::WrongKey)
    );

    let bytes = key.seal_bytes("two_factor.1", &TWO_FACTOR).unwrap();
    assert_eq!(bytes.len(), 49, "what the two_factor table's check allows");
    assert_eq!(bytes[0], 1);
    assert_eq!(key.open_bytes("two_factor.1", &bytes).unwrap(), TWO_FACTOR);
    assert_eq!(
        key.open_bytes("two_factor.2", &bytes),
        Err(Unreadable::WrongKey)
    );
    assert_eq!(
        format!("{:?}", other_key()),
        "Key(<redacted>)",
        "a key never shows in Debug output"
    );
}

#[tokio::test]
async fn the_first_start_makes_the_key_and_seals_what_was_plain_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let pool = plain_database(dir.path()).await;
    let key = prepare(&pool, dir.path()).await.unwrap();

    // The key file: 64 hex characters, for Melete's user alone.
    let file = dir.path().join(KEY_FILE);
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.trim_end().len(), 64);
    assert!(text.trim_end().bytes().all(|b| b.is_ascii_hexdigit()));
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    let (token, indexer, two_factor) = raw(&pool).await;
    assert!(token.starts_with("sealed:v1:") && !token.contains(TOKEN));
    assert!(indexer.starts_with("sealed:v1:") && !indexer.contains(INDEXER));
    assert_eq!(two_factor.len(), 49);
    assert_eq!(
        key.open_text(&setting("MELETE_TMDB_TOKEN"), &token)
            .unwrap(),
        TOKEN
    );
    assert_eq!(key.open_text(INDEXER_KEY, &indexer).unwrap(), INDEXER);
    let (link, guid) = links(&pool).await;
    assert!(link.starts_with("sealed:v1:") && !link.contains("plain-link-key"));
    assert!(guid.starts_with("sealed:v1:") && !guid.contains("plain-passkey"));
    assert_eq!(key.open_text(DOWNLOAD_URL, &link).unwrap(), LINK);
    assert_eq!(key.open_text(super::GUID, &guid).unwrap(), GUID);
    let plain: (String, String) = sqlx::query_as(
        "SELECT (SELECT value FROM settings WHERE name = 'MELETE_LYRICS'),
                (SELECT api_key FROM indexers WHERE id = 2)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        plain,
        ("lrclib".into(), String::new()),
        "what isn't secret stays"
    );

    // The next start reads the same key and leaves the sealed values as they are.
    let before = (raw(&pool).await, links(&pool).await);
    let again = prepare(&pool, dir.path()).await.unwrap();
    assert_eq!(
        (raw(&pool).await, links(&pool).await),
        before,
        "sealed once, not again"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        text,
        "the same key"
    );
    assert_eq!(again.open_text(INDEXER_KEY, &indexer).unwrap(), INDEXER);
    assert_eq!(switch(&pool, &again).await.unwrap(), None);
}

#[tokio::test]
async fn a_switched_database_refuses_plain_secrets() {
    let pool = db::open_in_memory().await;
    switch(&pool, key().unwrap()).await.unwrap().unwrap();
    for refused in [
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_SLSKD_API_KEY', 'plain', 0)",
        "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
             tv_search_params, source, added_at) VALUES ('x', 'u', 'plain', 1, '[]', '[]', '[]', 'melete', 0)",
        "DELETE FROM secrets_sealed",
    ] {
        let err = sqlx::query(refused).execute(&pool).await.unwrap_err();
        assert!(err.to_string().contains("sealed"), "{refused}: {err}");
    }
    // What isn't a secret, and an indexer that needs no key, are kept as they are.
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_SLSKD_URL', 'http://slskd:5030', 0);
         INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
             tv_search_params, source, added_at) VALUES ('x', 'u', '', 1, '[]', '[]', '[]', 'melete', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let user = auth::create_user(&pool, "alice", auth::generate_password().unwrap(), false)
        .await
        .unwrap()
        .unwrap();
    let err = sqlx::query("INSERT INTO two_factor (user_id, secret, created_at) VALUES (?, ?, 0)")
        .bind(user.id)
        .bind(&TWO_FACTOR[..])
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("sealed"), "{err}");
}

/// The database refuses plain values for exactly the settings the catalogue calls secret.
#[tokio::test]
async fn the_database_knows_the_catalogues_secrets() {
    let pool = db::open_in_memory().await;
    switch(&pool, key().unwrap()).await.unwrap().unwrap();
    for field in settings::SERVICES.iter().flat_map(|s| s.fields.iter()) {
        let stored =
            sqlx::query("INSERT INTO settings (name, value, updated_at) VALUES (?, 'x', 0)")
                .bind(field.name)
                .execute(&pool)
                .await;
        assert_eq!(
            stored.is_err(),
            settings::is_secret(field.name),
            "{}",
            field.name
        );
    }
}

#[tokio::test]
async fn a_plain_value_slipped_into_a_switched_database_is_refused_without_showing_it() {
    let pool = db::open_in_memory().await;
    switch(&pool, key().unwrap()).await.unwrap().unwrap();
    // As if written past the triggers.
    sqlx::query("DROP TRIGGER settings_secret_sealed_insert")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TMDB_TOKEN', ?, 0)",
    )
    .bind(TOKEN)
    .execute(&pool)
    .await
    .unwrap();
    let err = settings::value(&pool, "MELETE_TMDB_TOKEN")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("MELETE_TMDB_TOKEN") && err.contains("unsealed"),
        "{err}"
    );
    assert!(!err.contains(TOKEN), "{err}");
    assert!(settings::stored(&pool).await.is_err());
    let found = unreadable(&pool, Some(key().unwrap())).await.unwrap();
    assert_eq!(
        found.settings,
        [("MELETE_TMDB_TOKEN".to_owned(), Unreadable::Unsealed)]
    );
}

#[tokio::test]
async fn a_start_refused_for_unsealed_values_says_so_not_that_the_key_is_wrong() {
    let pool = db::open_in_memory().await;
    switch(&pool, key().unwrap()).await.unwrap().unwrap();
    sqlx::query("DROP TRIGGER settings_secret_sealed_insert")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TMDB_TOKEN', ?, 0)",
    )
    .bind(TOKEN)
    .execute(&pool)
    .await
    .unwrap();
    // The data folder holds the very key the tests seal with.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(KEY_FILE), "07".repeat(32)).unwrap();
    let said = prepare(&pool, dir.path()).await.unwrap_err().to_string();
    assert!(said.contains("kept unsealed in a database"), "{said}");
    assert!(said.contains("MELETE_TMDB_TOKEN (kept unsealed)"), "{said}");
    assert!(
        !said.contains("isn't the key they were sealed with"),
        "{said}"
    );
    assert!(!said.contains("put back"), "the key is fine: {said}");
    assert!(!said.contains(TOKEN));
}

#[tokio::test]
async fn a_key_file_cut_short_before_anything_was_sealed_is_safe_to_delete() {
    let pool = db::open_in_memory().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(KEY_FILE), "").unwrap();
    let said = prepare(&pool, dir.path()).await.unwrap_err().to_string();
    assert!(
        said.contains("safe to delete it and start Melete again"),
        "{said}"
    );
    assert!(
        std::fs::read(dir.path().join(KEY_FILE)).unwrap().is_empty(),
        "never made over"
    );

    // A new key is written beside its place first; one left by a stopped start is never used.
    let dir = tempfile::tempdir().unwrap();
    let temp = dir.path().join(".secrets.key.new");
    std::fs::write(&temp, "left by a start that stopped").unwrap();
    let key = prepare(&pool, dir.path()).await.unwrap();
    assert!(!temp.exists());
    let text = std::fs::read_to_string(dir.path().join(KEY_FILE)).unwrap();
    assert_eq!(text.trim_end().len(), 64);
    let sealed = key.seal_text("settings.A", TOKEN).unwrap();
    let again = prepare(&pool, dir.path()).await.unwrap();
    assert_eq!(again.open_text("settings.A", &sealed).unwrap(), TOKEN);
}

#[tokio::test]
async fn a_missing_or_wrong_key_stops_the_start_saying_which_and_how_to_recover() {
    let dir = tempfile::tempdir().unwrap();
    let pool = plain_database(dir.path()).await;
    prepare(&pool, dir.path()).await.unwrap();
    let file = dir.path().join(KEY_FILE);
    let kept = std::fs::read_to_string(&file).unwrap();

    std::fs::remove_file(&file).unwrap();
    let missing = prepare(&pool, dir.path()).await.unwrap_err().to_string();
    assert!(missing.contains("isn't there"), "{missing}");
    assert!(!file.exists(), "no new key over sealed secrets");
    for said in [
        "MELETE_TMDB_TOKEN",
        "Download links of 1 search results",
        "Keyed",
        "alice",
        "put back the secrets.key",
        "melete secrets clear",
        "melete user reset-password <name> --remove-second-factor",
    ] {
        assert!(missing.contains(said), "{said}: {missing}");
    }
    assert!(!missing.contains("Keyless"), "{missing}");
    assert!(!missing.contains(TOKEN) && !missing.contains(INDEXER));

    std::fs::write(&file, "ab".repeat(32)).unwrap();
    let wrong = prepare(&pool, dir.path()).await.unwrap_err().to_string();
    assert!(
        wrong.contains("isn't the key they were sealed with"),
        "{wrong}"
    );
    assert!(wrong.contains("MELETE_TMDB_TOKEN") && wrong.contains("alice"));

    std::fs::write(&file, "not a key").unwrap();
    let garbled = prepare(&pool, dir.path()).await.unwrap_err().to_string();
    assert!(garbled.contains("64 hexadecimal characters"), "{garbled}");

    // Putting the key back is one way back.
    std::fs::write(&file, &kept).unwrap();
    prepare(&pool, dir.path()).await.unwrap();
}

#[tokio::test]
async fn clearing_what_cant_be_read_lets_melete_start_with_a_new_key() {
    let dir = tempfile::tempdir().unwrap();
    let pool = plain_database(dir.path()).await;
    prepare(&pool, dir.path()).await.unwrap();
    let file = dir.path().join(KEY_FILE);
    std::fs::remove_file(&file).unwrap();

    let cleared = clear(&pool, dir.path()).await.unwrap();
    assert_eq!(
        cleared,
        Unreadables {
            settings: vec![("MELETE_TMDB_TOKEN".into(), Unreadable::NoKey)],
            indexers: vec![(1, "Keyed".into(), Unreadable::NoKey)],
            two_factor: vec![("alice".into(), Unreadable::NoKey)],
            setups: vec![],
            links: vec![1],
        }
    );
    let left: (i64, String, String) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM settings WHERE name = 'MELETE_TMDB_TOKEN'),
                (SELECT api_key FROM indexers WHERE id = 1),
                (SELECT value FROM settings WHERE name = 'MELETE_LYRICS')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(left, (0, String::new(), "lrclib".into()));
    let gone: bool = sqlx::query_scalar(
        "SELECT download_url IS NULL AND guid IS NULL FROM release_decisions WHERE id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(gone, "a search result's link, found again by a new search");
    // Two-factor is left to `user reset-password`, so Melete still won't start.
    let still = prepare(&pool, dir.path()).await.unwrap_err().to_string();
    assert!(
        still.contains("alice") && !still.contains("MELETE_TMDB_TOKEN"),
        "{still}"
    );
    let alice = auth::find_user(&pool, "alice").await.unwrap().unwrap();
    crate::two_factor::remove(&pool, alice.id, crate::two_factor::MadeFrom::Server)
        .await
        .unwrap();
    prepare(&pool, dir.path()).await.unwrap();
    assert!(file.exists(), "a new key once nothing sealed is left");
}

#[tokio::test]
async fn a_backup_holds_no_secret_readable_without_the_key_file() {
    let dir = tempfile::tempdir().unwrap();
    let pool = plain_database(dir.path()).await;
    let key = prepare(&pool, dir.path()).await.unwrap();
    let copy = crate::backup::by_hand(&pool, dir.path()).await.unwrap();
    let path = crate::backup::folder(dir.path()).unwrap().join(&copy.file);
    let bytes = std::fs::read(&path).unwrap();
    for plain in [TOKEN, INDEXER, "plain-link-key", "plain-passkey"]
        .map(str::as_bytes)
        .into_iter()
        .chain([&TWO_FACTOR[..]])
    {
        assert!(
            !bytes.windows(plain.len()).any(|w| w == plain),
            "a plain secret in the backup"
        );
    }
    // The copy's sealed values open with the key file, and with no other key.
    let copy_dir = tempfile::tempdir().unwrap();
    std::fs::copy(&path, copy_dir.path().join("melete.db")).unwrap();
    let restored = db::open(copy_dir.path()).await.unwrap();
    let (token, indexer, _) = raw(&restored).await;
    assert_eq!(
        other_key().open_text(&setting("MELETE_TMDB_TOKEN"), &token),
        Err(Unreadable::WrongKey)
    );
    assert_eq!(key.open_text(INDEXER_KEY, &indexer).unwrap(), INDEXER);
    assert!(prepare(&restored, copy_dir.path()).await.is_err());
}

#[tokio::test]
async fn settings_are_saved_sealed_and_read_back_whole() {
    let pool = db::open_in_memory().await;
    switch(&pool, key().unwrap()).await.unwrap().unwrap();
    let env = |_: &str| None;
    let change: HashMap<String, Option<String>> = [
        (
            "MELETE_SLSKD_URL".to_owned(),
            Some("http://slskd:5030".to_owned()),
        ),
        (
            "MELETE_SLSKD_API_KEY".to_owned(),
            Some("slskd-key-value".to_owned()),
        ),
    ]
    .into();
    settings::save(&pool, "slskd", &change, &env)
        .await
        .unwrap()
        .unwrap();
    let raw: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE name = 'MELETE_SLSKD_API_KEY'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(raw.starts_with("sealed:v1:") && !raw.contains("slskd-key-value"));
    let stored = settings::stored(&pool).await.unwrap();
    assert_eq!(stored["MELETE_SLSKD_API_KEY"], "slskd-key-value");
    assert_eq!(stored["MELETE_SLSKD_URL"], "http://slskd:5030");
    assert_eq!(
        settings::value(&pool, "MELETE_SLSKD_API_KEY")
            .await
            .unwrap()
            .as_deref(),
        Some("slskd-key-value")
    );
    let config = crate::config::Config::from_env_and(settings::lookup(&stored, &env)).unwrap();
    assert_eq!(config.slskd.unwrap().api_key.expose(), "slskd-key-value");
    // Put back after a change that couldn't be set up: sealed again.
    settings::restore(&pool, ["MELETE_SLSKD_API_KEY".to_owned()].iter(), &stored)
        .await
        .unwrap();
    assert_eq!(
        settings::stored(&pool).await.unwrap()["MELETE_SLSKD_API_KEY"],
        "slskd-key-value"
    );
}

#[tokio::test]
async fn indexer_keys_are_kept_sealed_and_searches_get_them_whole() {
    use crate::import::testing::FakeArr;

    let pool = db::open_in_memory().await;
    switch(&pool, key().unwrap()).await.unwrap().unwrap();
    let listed: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/prowlarr/indexers.json")).unwrap();
    let fake = FakeArr::default();
    fake.reply(
        "/api/v1/system/status",
        serde_json::json!({ "appName": "Prowlarr" }),
    )
    .reply("/api/v1/indexer", listed);
    let client = crate::arr::ArrClient::new(&fake.start().await, "key".into()).unwrap();
    crate::indexers::import_prowlarr(&pool, &client)
        .await
        .unwrap();
    let raw: Vec<String> = sqlx::query_scalar("SELECT api_key FROM indexers")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(!raw.is_empty());
    assert!(raw.iter().all(|k| k.starts_with("sealed:v1:")), "{raw:?}");
    let searched = crate::search::enabled_indexers(&pool).await.unwrap();
    assert!(!searched.is_empty());
    assert!(searched.iter().all(|i| i.api_key == "key"));
}

mod upgrade;
