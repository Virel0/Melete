//! The upgrade as an install lives it: no plain secret left in the database file, its WAL or a
//! backup copy, and the printed way back after a lost key working to the end.

use std::path::Path;

use sqlx::migrate::Migrator;
use sqlx::{ConnectOptions, Connection};

use super::*;
use crate::cli::{SecondFactor, reset_password};

/// Whether `bytes` hold `plain` anywhere.
fn holds(bytes: &[u8], plain: &[u8]) -> bool {
    bytes.windows(plain.len()).any(|w| w == plain)
}

/// The plain samples no file may hold after the switch.
fn samples() -> Vec<&'static [u8]> {
    vec![
        TOKEN.as_bytes(),
        INDEXER.as_bytes(),
        b"plain-link-key",
        b"plain-passkey",
        &TWO_FACTOR[..],
    ]
}

/// Every file of the data folder that holds database pages: the database, its WAL, and each
/// backup copy.
fn database_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = vec![dir.join("melete.db"), dir.join("melete.db-wal")];
    let backups = crate::backup::folder(dir).unwrap();
    for entry in std::fs::read_dir(backups).unwrap() {
        files.push(entry.unwrap().path());
    }
    files.into_iter().filter(|f| f.exists()).collect()
}

#[tokio::test]
async fn after_the_switch_no_file_holds_a_plain_secret() {
    let dir = tempfile::tempdir().unwrap();
    let pool = plain_database(dir.path()).await;
    // A copy made by hand before the switch holds them plain, as the daily ones do.
    let copy = crate::backup::by_hand(&pool, dir.path()).await.unwrap();
    let path = crate::backup::folder(dir.path()).unwrap().join(&copy.file);
    assert!(holds(&std::fs::read(&path).unwrap(), TOKEN.as_bytes()));

    let key = prepare(&pool, dir.path()).await.unwrap();
    for file in database_files(dir.path()) {
        let bytes = std::fs::read(&file).unwrap();
        for plain in samples() {
            assert!(
                !holds(&bytes, plain),
                "{} holds a plain secret",
                file.display()
            );
        }
    }
    // Each copy still opens, and its secrets with the key.
    for (name, _, _) in crate::backup::restore::copies(dir.path()).unwrap() {
        let path = crate::backup::folder(dir.path()).unwrap().join(&name);
        let mut copy = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
            .connect()
            .await
            .unwrap();
        let token: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE name = 'MELETE_TMDB_TOKEN'")
                .fetch_one(&mut copy)
                .await
                .unwrap();
        assert_eq!(
            key.open_text(&setting("MELETE_TMDB_TOKEN"), &token)
                .unwrap(),
            TOKEN,
            "{name}"
        );
        let ok: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&mut copy)
            .await
            .unwrap();
        assert_eq!(ok, "ok", "{name}");
        copy.close().await.unwrap();
    }
}

#[tokio::test]
async fn a_copy_from_before_sealed_storage_is_sealed_too() {
    let dir = tempfile::tempdir().unwrap();
    let all = sqlx::migrate!();
    let before = Migrator {
        migrations: all
            .migrations
            .iter()
            .filter(|m| !m.description.contains("sealed secrets"))
            .cloned()
            .collect(),
        ..Migrator::DEFAULT
    };
    // The old version, holding plain secrets, two-factor included.
    let old = db::open_with(dir.path(), &before).await.unwrap();
    let alice = auth::create_user(&old, "alice", auth::generate_password().unwrap(), false)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TMDB_TOKEN', ?, 0);
         INSERT INTO two_factor (user_id, secret, created_at, confirmed_at) VALUES (?, ?, 0, 0)",
    )
    .bind(TOKEN)
    .bind(alice.id)
    .bind(&TWO_FACTOR[..])
    .execute(&old)
    .await
    .unwrap();
    old.close().await;
    // Upgrading copies it before migrating, as every upgrade does.
    let pool = db::open(dir.path()).await.unwrap();
    let copies = crate::backup::restore::copies(dir.path()).unwrap();
    assert!(
        copies
            .iter()
            .any(|(n, _, _)| n.starts_with("before-migrating-from-"))
    );
    let changed: Vec<_> = database_files(dir.path())
        .iter()
        .filter(|f| f.starts_with(crate::backup::folder(dir.path()).unwrap()))
        .map(|f| std::fs::metadata(f).unwrap().modified().unwrap())
        .collect();

    let key = prepare(&pool, dir.path()).await.unwrap();
    for file in database_files(dir.path()) {
        let bytes = std::fs::read(&file).unwrap();
        for plain in [TOKEN.as_bytes(), &TWO_FACTOR[..]] {
            assert!(
                !holds(&bytes, plain),
                "{} holds a plain secret",
                file.display()
            );
        }
    }
    let kept: Vec<_> = database_files(dir.path())
        .iter()
        .filter(|f| f.starts_with(crate::backup::folder(dir.path()).unwrap()))
        .map(|f| std::fs::metadata(f).unwrap().modified().unwrap())
        .collect();
    assert_eq!(kept, changed, "a copy keeps the time it was made");

    // Restored, the old copy migrates and opens with the key, two-factor included.
    let (name, _, _) = &copies[0];
    let restored = tempfile::tempdir().unwrap();
    std::fs::copy(
        crate::backup::folder(dir.path()).unwrap().join(name),
        restored.path().join("melete.db"),
    )
    .unwrap();
    std::fs::copy(dir.path().join(KEY_FILE), restored.path().join(KEY_FILE)).unwrap();
    let again = db::open(restored.path()).await.unwrap();
    prepare(&again, restored.path()).await.unwrap();
    let secret: Vec<u8> = sqlx::query_scalar("SELECT secret FROM two_factor")
        .fetch_one(&again)
        .await
        .unwrap();
    assert_eq!(
        key.open_bytes(&super::super::two_factor(alice.id), &secret)
            .unwrap(),
        TWO_FACTOR
    );
}

#[tokio::test]
async fn the_printed_way_back_after_a_lost_key_lets_melete_start() {
    let dir = tempfile::tempdir().unwrap();
    let pool = plain_database(dir.path()).await;
    // Bob started setting up two-factor sign-in and never typed his first code.
    let bob = auth::create_user(&pool, "bob", auth::generate_password().unwrap(), false)
        .await
        .unwrap()
        .unwrap();
    crate::two_factor::start_setup(&pool, &bob)
        .await
        .unwrap()
        .unwrap();
    prepare(&pool, dir.path()).await.unwrap();
    std::fs::remove_file(dir.path().join(KEY_FILE)).unwrap();

    let said = prepare(&pool, dir.path()).await.unwrap_err().to_string();
    assert!(said.contains("Two-factor sign-in of: alice"), "{said}");
    assert!(
        said.contains("Unconfirmed two-factor setups of: bob"),
        "{said}"
    );
    // The steps it prints: clear, then reset each user named under two-factor sign-in.
    let cleared = clear(&pool, dir.path()).await.unwrap();
    assert_eq!(cleared.setups, [(bob.id, "bob".to_owned())]);
    let never_asked = || -> anyhow::Result<bool> { panic!("asked") };
    reset_password(&pool, "alice", SecondFactor::Remove, never_asked)
        .await
        .unwrap();
    prepare(&pool, dir.path()).await.unwrap();
    assert!(dir.path().join(KEY_FILE).exists(), "a new key");
    assert_eq!(
        crate::two_factor::stage(&pool, bob.id).await.unwrap(),
        crate::two_factor::Stage::Off,
        "bob can start again"
    );
}
