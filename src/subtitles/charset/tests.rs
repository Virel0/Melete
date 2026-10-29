use super::*;

/// An SRT file of these lines in `encoding`, named `name`, in `dir`.
fn srt_in(dir: &Path, name: &str, encoding: &'static Encoding, lines: &[&str]) -> PathBuf {
    let mut text = String::new();
    for (i, line) in lines.iter().enumerate() {
        text.push_str(&format!(
            "{}\n00:00:{:02},000 --> 00:00:{:02},500\n{line}\n\n",
            i + 1,
            i * 2,
            i * 2 + 1
        ));
    }
    let (bytes, _, unmappable) = encoding.encode(&text);
    assert!(!unmappable, "{} can carry the text", encoding.name());
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Converts the file to WebVTT and checks every line came through as written, and that the
/// subtitle list would say what it was read as.
async fn converts(dir: &Path, path: &Path, encoding: &'static Encoding, lines: &[&str]) {
    let key = path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .replace('.', "-");
    let vtt = webvtt(
        Path::new("ffmpeg"),
        &dir.join("cache"),
        &key,
        Source::Sidecar(path),
    )
    .await
    .unwrap();
    let vtt = String::from_utf8(vtt).expect("WebVTT is UTF-8");
    for line in lines {
        assert!(vtt.contains(line), "{line} in {vtt}");
    }
    assert!(vtt.starts_with("WEBVTT"), "{vtt}");
    assert_eq!(file_encoding(path), Some(encoding.name()));
}

#[tokio::test]
async fn subtitles_in_legacy_encodings_convert_with_their_own_characters() {
    let dir = tempfile::tempdir().unwrap();
    let cases: [(&str, &'static Encoding, &[&str]); 4] = [
        (
            "Movie.ru.srt",
            encoding_rs::WINDOWS_1251,
            &[
                "Привет, как у тебя дела сегодня?",
                "Это проверка русских субтитров.",
                "Мы поедем в город завтра утром.",
            ],
        ),
        (
            "Movie.fr.srt",
            encoding_rs::WINDOWS_1252,
            &[
                "Ça va très bien, merci beaucoup.",
                "L'été dernier, nous étions à Paris.",
                "Où est la gare? Je ne sais pas, désolé.",
            ],
        ),
        (
            "Movie.zh.srt",
            encoding_rs::GBK,
            &[
                "你好，这是中文字幕的测试。",
                "我们今天晚上一起去看电影吧。",
                "明天的天气怎么样？",
            ],
        ),
        (
            "Movie.ja.srt",
            encoding_rs::SHIFT_JIS,
            &[
                "こんにちは、これは日本語の字幕です。",
                "今日はとても良い天気ですね。",
                "明日、一緒に映画を見に行きましょう。",
            ],
        ),
    ];
    for (name, encoding, lines) in cases {
        let path = srt_in(dir.path(), name, encoding, lines);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(guess_encoding(&bytes, name), encoding, "{name}");
        converts(dir.path(), &path, encoding, lines).await;
    }
}

#[tokio::test]
async fn a_byte_order_mark_says_the_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let lines = ["Zażółć gęślą jaźń"];
    let path = dir.path().join("Movie.pl.srt");
    let text = "1\n00:00:00,000 --> 00:00:01,500\nZażółć gęślą jaźń\n";
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        guess_encoding(&bytes, "Movie.pl.srt"),
        encoding_rs::UTF_16LE
    );
    converts(dir.path(), &path, encoding_rs::UTF_16LE, &lines).await;
}

#[tokio::test]
async fn utf8_is_read_as_it_is_with_or_without_a_mark() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    for (name, mark) in [
        ("plain.ru.srt", &b""[..]),
        ("marked.ru.srt", b"\xef\xbb\xbf"),
    ] {
        let path = dir.path().join(name);
        let mut bytes = mark.to_vec();
        bytes.extend("1\n00:00:00,000 --> 00:00:01,000\nПривет\n".as_bytes());
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(guess_encoding(&bytes, name), UTF_8);
        assert_eq!(to_utf8(&bytes, name).1, None, "nothing to convert");
        let vtt = webvtt(Path::new("ffmpeg"), &cache, name, Source::Sidecar(&path))
            .await
            .unwrap();
        let vtt = String::from_utf8(vtt).unwrap();
        assert!(vtt.contains("Привет") && !vtt.contains("NOTE"), "{vtt}");
    }
    let left: Vec<_> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n.to_string_lossy().ends_with(".part"))
        .collect();
    assert!(left.is_empty(), "no copies left behind: {left:?}");
}

#[test]
fn the_language_in_the_name_is_a_hint() {
    assert_eq!(language_hint("Movie.pl.srt"), Some(&b"pl"[..]));
    assert_eq!(language_hint("Movie.POL.forced.srt"), Some(&b"pl"[..]));
    assert_eq!(language_hint("Movie.jpn.sdh.ass"), Some(&b"jp"[..]));
    assert_eq!(language_hint("Movie.en.srt"), None);
    assert_eq!(language_hint("Movie.zh.srt"), None, "GBK or Big5");
    assert_eq!(language_hint("ru"), None, "the extension alone isn't a tag");
    // A short line of Central European text is taken for Western European without its
    // language, and read right with it.
    let text = "Proszę, zamknij drzwi. Dziękuję bardzo.";
    let (bytes, _, _) = encoding_rs::WINDOWS_1250.encode(text);
    let read_as = |name: &str| to_utf8(&bytes, name).1.unwrap();
    assert_eq!(read_as("Movie.pl.srt"), text);
    assert_ne!(read_as("Movie.srt"), text);
}

#[test]
fn a_file_too_big_to_be_subtitles_has_no_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Movie.en.srt");
    std::fs::write(&path, vec![b'a'; SUBTITLE_FILE_LIMIT as usize + 1]).unwrap();
    assert_eq!(file_encoding(&path), None);
    assert_eq!(file_encoding(&dir.path().join("gone.srt")), None);
}

#[tokio::test]
async fn copies_cached_before_encodings_were_read_are_cleaned_up() {
    let library = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(library.path().join("Movie.mkv"), b"video").unwrap();
    std::fs::write(library.path().join("Movie.ru.srt"), b"\xcf\xf0\xe8").unwrap();
    let pool = crate::db::open_in_memory().await;
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?)")
        .bind(library.path().to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'Movie', 'Movie', 1)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id, first_seen_generation, seen_generation)
         VALUES (7, 1, 'Movie.mkv', 5, 100, 1, 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let sidecar = sidecars(&library.path().join("Movie.mkv"))
        .unwrap()
        .remove(0);
    let current = sidecar_key(7, &sidecar);
    let garbled = current.strip_suffix("-u").expect("keys end in -u");
    for name in [format!("{current}.vtt"), format!("{garbled}.vtt")] {
        std::fs::write(cache.path().join(name), b"WEBVTT").unwrap();
    }
    assert_eq!(clean_cache(&pool, cache.path()).await.unwrap(), 1);
    assert!(cache.path().join(format!("{current}.vtt")).exists());
}
