use super::*;

fn chapter(start: f64, end: f64, title: Option<&str>) -> Chapter {
    Chapter {
        start,
        end,
        title: title.map(Into::into),
    }
}

#[test]
fn chapters_are_kept_as_they_are_and_none_are_made_up() {
    let json = br#"{"chapters": [
        {"start_time": "0.000000", "end_time": "90.500000", "tags": {"title": "Opening"}},
        {"start_time": "90.500000", "end_time": "90.500000", "tags": {"title": "Nothing"}},
        {"start_time": "90.500000", "end_time": "1300.000000", "tags": {"TITLE": " Part A "}},
        {"start_time": "40.000000", "end_time": "50.000000", "tags": {"title": "Out of order"}},
        {"start_time": "1300.000000", "end_time": "1390.000000", "tags": {"title": ""}},
        {"end_time": "1400.000000"},
        {"start_time": "1390.000000", "end_time": "1420.000000"}
    ]}"#;
    assert_eq!(
        parse_chapters(json).unwrap(),
        vec![
            chapter(0.0, 90.5, Some("Opening")),
            chapter(90.5, 1300.0, Some("Part A")),
            chapter(1300.0, 1390.0, None),
            chapter(1390.0, 1420.0, None),
        ]
    );
    assert_eq!(parse_chapters(br#"{"format": {}}"#).unwrap(), []);
    assert!(parse_chapters(b"not json").is_err());
}

#[test]
fn intros_and_credits_are_known_by_their_names() {
    for intro in [
        "Opening",
        "OP",
        "op1",
        "Intro",
        "OPENING CREDITS",
        "Opening 2",
        "Main Titles",
    ] {
        assert_eq!(chapter_kind(intro), Some(ChapterKind::Intro), "{intro}");
    }
    for credits in [
        "Ending",
        "ED",
        "ED2",
        "Credits",
        "End Credits",
        "Ending Theme",
        "Outro",
    ] {
        assert_eq!(
            chapter_kind(credits),
            Some(ChapterKind::Credits),
            "{credits}"
        );
    }
    for neither in [
        "Chapter 1",
        "Part A",
        "Prologue",
        "Opening Scene",
        "Preview",
        "Opera",
    ] {
        assert_eq!(chapter_kind(neither), None, "{neither}");
    }
}

#[test]
fn the_first_intro_and_the_last_credits_count() {
    let chapters = vec![
        chapter(0.0, 60.0, Some("Recap")),
        chapter(60.0, 150.0, Some("OP")),
        chapter(150.0, 1200.0, Some("Part A")),
        chapter(1200.0, 1290.0, Some("Opening")),
        chapter(1290.0, 1300.0, Some("ED")),
        chapter(1300.0, 1380.0, Some("Ending")),
        chapter(1380.0, 1410.0, Some("Preview")),
    ];
    let (intro, credits) = named_segments(&chapters);
    assert_eq!(intro, Some(&chapters[1]));
    assert_eq!(credits, Some(&chapters[5]));
    assert_eq!(named_segments(&chapters[2..3]), (None, None));
}

#[test]
fn recaps_and_previews_are_known_by_their_names() {
    for recap in [
        "Recap",
        "RECAP 2",
        "Previously",
        "Previously on Severance",
        "Previously on...",
        "Last Time",
        "The Story So Far",
    ] {
        assert!(is_recap(recap), "{recap}");
        assert!(!is_preview(recap), "{recap}");
    }
    for preview in [
        "Preview",
        "Next Episode",
        "Next Episode Preview",
        "Next Time",
    ] {
        assert!(is_preview(preview), "{preview}");
        assert!(!is_recap(preview), "{preview}");
    }
    for neither in [
        "Prologue",
        "Part A",
        "Opening",
        "Previous",
        "Next",
        "Recaps Done",
    ] {
        assert!(!is_recap(neither) && !is_preview(neither), "{neither}");
    }
}

#[test]
fn the_first_recap_and_the_last_preview_count() {
    let chapters = vec![
        chapter(0.0, 40.0, Some("Previously on Show")),
        chapter(40.0, 60.0, Some("Recap")),
        chapter(60.0, 150.0, Some("OP")),
        chapter(150.0, 1300.0, Some("Part A")),
        chapter(1300.0, 1320.0, Some("Preview")),
        chapter(1320.0, 1380.0, Some("ED")),
        chapter(1380.0, 1410.0, Some("Next Time")),
    ];
    let (recap, preview) = named_recap_and_preview(&chapters);
    assert_eq!(recap, Some(&chapters[0]));
    assert_eq!(preview, Some(&chapters[6]));
    assert_eq!(named_recap_and_preview(&chapters[2..4]), (None, None));
}

/// A 30-second clip with the given chapters (start, end and title, in seconds), or none.
pub(crate) fn clip_with_chapters(dir: &Path, name: &str, chapters: &[(u32, u32, &str)]) -> PathBuf {
    let clip = dir.join(name);
    let metadata = dir.join(format!("{name}.txt"));
    let mut text = String::from(";FFMETADATA1\n");
    for (start, end, title) in chapters {
        text.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={title}\n",
            start * 1000,
            end * 1000
        ));
    }
    std::fs::write(&metadata, text).unwrap();
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=size=160x90:rate=24:duration=30")
        .arg("-i")
        .arg(&metadata)
        .args(["-map", "0", "-map_metadata", "1", "-map_chapters", "1"])
        .args(["-c:v", "libx264", "-preset", "ultrafast"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(made.success());
    clip
}

#[tokio::test]
async fn reads_a_files_chapters_and_a_file_without_any() {
    let dir = tempfile::tempdir().unwrap();
    let named = clip_with_chapters(
        dir.path(),
        "named.mkv",
        &[(0, 5, "Opening"), (5, 25, "Chapter 2"), (25, 30, "Ending")],
    );
    let probed = probe_with_chapters(Path::new("ffprobe"), &named)
        .await
        .unwrap();
    assert_eq!(probed.info.video.len(), 1);
    let chapters = probed.chapters.unwrap();
    let titles: Vec<_> = chapters.iter().map(|c| c.title.as_deref()).collect();
    assert_eq!(titles, [Some("Opening"), Some("Chapter 2"), Some("Ending")]);
    assert_eq!((chapters[1].start, chapters[1].end), (5.0, 25.0));

    let plain = clip_with_chapters(dir.path(), "plain.mkv", &[]);
    let probed = probe_with_chapters(Path::new("ffprobe"), &plain)
        .await
        .unwrap();
    assert_eq!(probed.chapters, Some(Vec::new()), "none made up");
}

#[tokio::test]
async fn files_probed_before_chapters_were_read_are_probed_once_more() {
    let dir = tempfile::tempdir().unwrap();
    let clip = clip_with_chapters(
        dir.path(),
        "clip.mkv",
        &[(0, 10, "Intro"), (10, 30, "Rest")],
    );
    let size = std::fs::metadata(&clip).unwrap().len() as i64;
    let pool = crate::db::open_in_memory().await;
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?)")
        .bind(dir.path().to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'clip', 'Clip', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (1, 1, 'clip.mkv', ?, 7, 1, 1, 1)",
    )
    .bind(size)
    .execute(&pool)
    .await
    .unwrap();
    // Probed by an older version: streams, but chapters never read.
    let info = probe(Path::new("ffprobe"), &clip).await;
    store(&pool, 1, size, 7, &info).await.unwrap();
    assert_eq!(chapters_of(&pool, 1).await.unwrap(), []);

    let report = probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
    assert_eq!(report.probed, 1);
    let chapters = chapters_of(&pool, 1).await.unwrap();
    assert_eq!(chapters[0], chapter(0.0, 10.0, Some("Intro")));
    let again = probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
    assert_eq!(again.probed, 0, "once");

    // Chapters belong to the version read: a changed file has none until probed again.
    sqlx::query("UPDATE media_files SET modified_at = 8 WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(chapters_of(&pool, 1).await.unwrap(), []);
}
