use std::time::Duration;

use super::*;

fn library(id: i64, kind: LibraryKind, path: &Path) -> Library {
    Library {
        id,
        name: format!("l{id}"),
        kind,
        path: path.to_path_buf(),
    }
}

fn ask(library_id: i64, folder: Option<&str>) -> Ask {
    Ask {
        library_id,
        folder: folder.map(str::to_owned),
    }
}

/// The next ask the watch sends, within a few seconds.
async fn next(asks: &mut UnboundedReceiver<Ask>) -> Ask {
    tokio::time::timeout(Duration::from_secs(5), asks.recv())
        .await
        .expect("the change was seen")
        .expect("the watch is still running")
}

#[tokio::test]
async fn a_change_asks_for_a_scan_of_what_it_touched() {
    let dir = tempfile::tempdir().unwrap();
    let movies = dir.path().join("movies");
    let music = dir.path().join("music");
    std::fs::create_dir_all(movies.join("Old (2020)/Extras")).unwrap();
    std::fs::create_dir_all(music.join("Artist/Album")).unwrap();
    let (sink, mut asks) = unbounded_channel();
    let watcher = Watcher::start(
        &[
            library(1, LibraryKind::Movies, &movies),
            library(2, LibraryKind::Music, &music),
        ],
        sink,
    )
    .unwrap();
    assert_eq!(watcher.folders, 6);

    // A new title folder: that folder.
    std::fs::create_dir(movies.join("New (2021)")).unwrap();
    assert_eq!(next(&mut asks).await, ask(1, Some("New (2021)")));
    // A hidden part file being written asks nothing; the file it becomes does, as does a write
    // deep in a title folder that was there from the start.
    std::fs::write(movies.join("Old (2020)/.Old.mkv.1.melete-part"), b"x").unwrap();
    std::fs::rename(
        movies.join("Old (2020)/.Old.mkv.1.melete-part"),
        movies.join("Old (2020)/Old.mkv"),
    )
    .unwrap();
    assert_eq!(next(&mut asks).await, ask(1, Some("Old (2020)")));
    std::fs::write(movies.join("Old (2020)/Extras/Making of.mkv"), b"x").unwrap();
    assert_eq!(next(&mut asks).await, ask(1, Some("Old (2020)")));
    // A folder made after the watch started is watched too.
    std::fs::write(movies.join("New (2021)/New.mkv"), b"x").unwrap();
    assert_eq!(next(&mut asks).await, ask(1, Some("New (2021)")));
    // A title folder renamed: the whole library, so the scan can tell it's the same title.
    std::fs::rename(movies.join("Old (2020)"), movies.join("Old (2020) [x]")).unwrap();
    assert_eq!(next(&mut asks).await, ask(1, None));
    assert_eq!(next(&mut asks).await, ask(1, None));
    // Music is scanned whole.
    std::fs::write(music.join("Artist/Album/01.flac"), b"x").unwrap();
    assert_eq!(next(&mut asks).await, ask(2, None));

    // Stopped, it asks nothing more: its thread ends and lets the channel go.
    drop(watcher);
    tokio::time::sleep(Duration::from_millis(600)).await;
    std::fs::write(movies.join("New (2021)/Later.mkv"), b"x").unwrap();
    let after = tokio::time::timeout(Duration::from_secs(5), asks.recv()).await;
    assert_eq!(after, Ok(None));
}

#[tokio::test]
async fn asks_are_passed_on_once_after_a_quiet_spell() {
    let quiet = Duration::from_secs(1);
    let (sink, asks) = unbounded_channel();
    let passed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = passed.clone();
    let gathering = tokio::spawn(gather(asks, quiet, move |ask| {
        seen.lock().unwrap().push(ask);
    }));
    for _ in 0..3 {
        sink.send(ask(1, Some("Show"))).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    sink.send(ask(1, None)).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(passed.lock().unwrap().is_empty(), "still busy");
    tokio::time::sleep(quiet * 2).await;
    assert_eq!(
        *passed.lock().unwrap(),
        [ask(1, Some("Show")), ask(1, None)],
        "each once, in order"
    );
    drop(sink);
    gathering.await.unwrap();
}

#[test]
fn events_are_read_from_inotifys_layout() {
    let mut bytes = Vec::new();
    for (wd, mask, name) in [
        (3, libc::IN_CLOSE_WRITE, "a.mkv"),
        (4, libc::IN_IGNORED, ""),
    ] {
        let padded = if name.is_empty() { 0 } else { 16 };
        bytes.extend_from_slice(&i32::to_ne_bytes(wd));
        bytes.extend_from_slice(&u32::to_ne_bytes(mask));
        bytes.extend_from_slice(&u32::to_ne_bytes(0));
        bytes.extend_from_slice(&u32::to_ne_bytes(padded));
        let mut raw = name.as_bytes().to_vec();
        raw.resize(padded as usize, 0);
        bytes.extend_from_slice(&raw);
    }
    let events = inotify::events(&bytes);
    assert_eq!(events.len(), 2);
    assert_eq!((events[0].wd, events[0].name.as_str()), (3, "a.mkv"));
    assert_eq!((events[1].wd, events[1].name.as_str()), (4, ""));
}
