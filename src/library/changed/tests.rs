use super::*;

/// Sets when `path` (a folder or file) last changed.
fn set_time(path: &Path, at: i64) {
    let at = UNIX_EPOCH + Duration::from_secs(u64::try_from(at).unwrap());
    std::fs::File::open(path).unwrap().set_modified(at).unwrap();
}

/// When the library was last scanned whole in these tests: a day ago, everything in it older,
/// so what's changed now is newer.
static SCANNED: std::sync::LazyLock<i64> =
    std::sync::LazyLock::new(|| crate::db::unix_now() - 86_400);

fn library() -> (tempfile::TempDir, HashSet<String>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for folder in ["Show/Season 1", "Other/Season 2", ".melete-trash/1"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    std::fs::write(root.join("Show/Season 1/Show.S01E01.mkv"), b"x").unwrap();
    for path in [
        "Show/Season 1",
        "Show",
        "Other/Season 2",
        "Other",
        ".melete-trash/1",
        ".melete-trash",
        "",
    ] {
        set_time(&root.join(path), *SCANNED - 3600);
    }
    let known = ["Show", "Other"].map(str::to_owned).into();
    (dir, known)
}

#[test]
fn nothing_changed_asks_for_nothing() {
    let (dir, known) = library();
    let since = *SCANNED - SLACK;
    assert_eq!(
        changed(dir.path(), LibraryKind::Shows, since, &known),
        Ok(Changed::Nothing)
    );
    // Something in the trash isn't a title.
    set_time(&dir.path().join(".melete-trash/1"), *SCANNED + 10);
    assert_eq!(
        changed(dir.path(), LibraryKind::Shows, since, &known),
        Ok(Changed::Nothing)
    );
}

#[test]
fn an_episode_added_to_a_season_asks_for_its_shows_folder() {
    let (dir, known) = library();
    std::fs::write(dir.path().join("Show/Season 1/Show.S01E02.mkv"), b"x").unwrap();
    set_time(&dir.path().join("Show/Season 1"), *SCANNED + 10);
    assert_eq!(
        changed(dir.path(), LibraryKind::Shows, *SCANNED - SLACK, &known),
        Ok(Changed::Folders(vec!["Show".into()]))
    );
}

#[test]
fn a_title_new_at_the_top_or_any_music_change_asks_for_the_whole_library() {
    let (dir, known) = library();
    // Moved in from elsewhere, keeping its own older time: the library's folder changed.
    std::fs::create_dir(dir.path().join("Renamed")).unwrap();
    set_time(&dir.path().join("Renamed"), *SCANNED - 3600);
    assert_eq!(
        changed(dir.path(), LibraryKind::Shows, *SCANNED - SLACK, &known),
        Ok(Changed::Whole)
    );
    let (music, _) = library();
    set_time(&music.path().join("Other/Season 2"), *SCANNED + 10);
    assert_eq!(
        changed(
            music.path(),
            LibraryKind::Music,
            *SCANNED - SLACK,
            &HashSet::new()
        ),
        Ok(Changed::Whole)
    );
}

#[tokio::test]
async fn only_a_library_scanned_before_and_answering_is_looked_at() {
    let pool = crate::db::open_in_memory().await;
    let (dir, _) = library();
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', ?);
         INSERT INTO series (library_id, rel_path, parsed_title, seen_generation)
         VALUES (1, 'Show', 'Show', 1), (1, 'Other', 'Other', 1)",
    )
    .bind(dir.path().to_string_lossy().into_owned())
    .execute(&pool)
    .await
    .unwrap();
    let shows = Library {
        id: 1,
        name: "shows".into(),
        kind: LibraryKind::Shows,
        path: dir.path().to_path_buf(),
    };
    health::load(&pool, std::slice::from_ref(&shows))
        .await
        .unwrap();
    set_time(&dir.path().join("Show/Season 1"), *SCANNED + 10);
    // Never scanned: left for someone to ask.
    assert_eq!(look_at(&pool, &shows).await, Ok(Changed::Nothing));
    sqlx::query("UPDATE libraries SET last_scan_at = ? WHERE id = 1")
        .bind(*SCANNED)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        look_at(&pool, &shows).await,
        Ok(Changed::Folders(vec!["Show".into()]))
    );
    // Not responding: not read at all.
    health::mark_at(dir.path(), "its folder didn't answer within 15 seconds");
    let refused = look_at(&pool, &shows).await.unwrap_err();
    assert!(refused.contains("isn't responding"), "{refused}");
}
