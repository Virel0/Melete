use super::*;
use crate::db;

#[test]
fn states_come_in_their_order() {
    let facts = |on_disk, missing, under_way, monitored| Facts {
        on_disk,
        missing,
        under_way,
        monitored,
    };
    // What it lacks is on its way, whether or not something is here.
    assert_eq!(facts(false, 1, true, true).state(), State::Downloading);
    assert_eq!(facts(true, 2, true, true).state(), State::Downloading);
    // A download of something that isn't monitored, with nothing here yet.
    assert_eq!(facts(false, 0, true, false).state(), State::Downloading);
    // On disk with an upgrade on its way stays in the library.
    assert_eq!(facts(true, 0, true, true).state(), State::InLibrary);
    assert_eq!(facts(true, 3, false, true).state(), State::Missing);
    assert_eq!(facts(false, 1, false, true).state(), State::Missing);
    // On disk and not monitored is in the library.
    assert_eq!(facts(true, 0, false, false).state(), State::InLibrary);
    assert_eq!(facts(false, 0, false, true).state(), State::Unreleased);
    assert_eq!(facts(false, 0, false, false).state(), State::Unmonitored);
}

fn title(name: &str, state: State, size: i64, year: Option<i64>, added: i64) -> Title<()> {
    Title {
        wanted_id: None,
        title: name.into(),
        year,
        poster: None,
        state,
        monitored: state != State::Unmonitored,
        wanted: state != State::InLibrary,
        profile_id: None,
        profile: None,
        library: Some(if size > 5 { "big" } else { "small" }.into()),
        category: (size > 5).then(|| "kids".into()),
        size,
        added_at: Some(added),
        download: None,
        cutoff_unmet: None,
        detail: (),
    }
}

fn names<D>(page: &Page<D>) -> Vec<&str> {
    page.items.iter().map(|t| t.title.as_str()).collect()
}

#[test]
fn pages_are_filtered_sorted_and_counted() {
    let all = || {
        vec![
            title("beta", State::Missing, 1, Some(2001), 30),
            title("Alpha", State::InLibrary, 9, None, 10),
            title("gamma", State::InLibrary, 7, Some(1999), 20),
            title("Delta", State::Unmonitored, 0, Some(2010), 40),
        ]
    };
    let page = page(all(), &Filter::default(), 2, 1);
    assert_eq!(page.total, 4);
    assert_eq!(names(&page), ["beta", "Delta"], "by title, case aside");
    assert_eq!(
        page.counts,
        Counts {
            in_library: 2,
            missing: 1,
            unmonitored: 1,
            ..Counts::default()
        }
    );
    let with = |filter: Filter| names(&super::page(all(), &filter, 10, 0)).join(",");
    let sorted = |sort| {
        with(Filter {
            sort,
            ..Filter::default()
        })
    };
    assert_eq!(sorted(Sort::Added), "Delta,beta,gamma,Alpha");
    assert_eq!(sorted(Sort::Year), "Delta,beta,gamma,Alpha", "no year last");
    assert_eq!(sorted(Sort::Size), "Alpha,gamma,beta,Delta");
    let state = Filter {
        state: Some(State::InLibrary),
        ..Filter::default()
    };
    assert_eq!(with(state), "Alpha,gamma");
    let monitored = Filter {
        monitored: Some(false),
        ..Filter::default()
    };
    assert_eq!(with(monitored), "Delta");
    let wanted = Filter {
        wanted: Some(true),
        ..Filter::default()
    };
    assert_eq!(with(wanted), "beta,Delta");
    let library = Filter {
        library: Some("big".into()),
        ..Filter::default()
    };
    assert_eq!(with(library), "Alpha,gamma");
    let none = Filter {
        category: Some("none".into()),
        ..Filter::default()
    };
    assert_eq!(with(none), "beta,Delta");
    let q = Filter {
        q: Some("ALP".into()),
        ..Filter::default()
    };
    assert_eq!(with(q), "Alpha");
    let filtered = super::page(
        all(),
        &Filter {
            state: Some(State::Missing),
            ..Filter::default()
        },
        10,
        0,
    );
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.counts.in_library, 2, "counts ignore the filter");
}

#[test]
fn titles_sort_without_their_article_and_are_found_without_accents() {
    let all = || {
        vec![
            title("The Matrix", State::InLibrary, 1, None, 0),
            title("Amélie", State::InLibrary, 1, None, 0),
            title("Spider-Man", State::InLibrary, 1, None, 0),
            title("Brazil", State::InLibrary, 1, None, 0),
        ]
    };
    let sorted = page(all(), &Filter::default(), 10, 0);
    assert_eq!(
        names(&sorted),
        ["Amélie", "Brazil", "The Matrix", "Spider-Man"]
    );
    for (typed, found) in [("amelie", "Amélie"), ("spiderman", "Spider-Man")] {
        let search = Filter {
            q: Some(typed.into()),
            ..Filter::default()
        };
        assert_eq!(names(&page(all(), &search, 10, 0)), [found], "{typed}");
    }
}

/// A library of each kind, a profile, and a TMDB movie out long ago and one out next year.
async fn fixture() -> SqlitePool {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path, category) VALUES
             (1, 'Films', 'movies', '/m', NULL), (2, 'Shows', 'shows', '/s', NULL),
             (3, 'Anime', 'shows', '/a', 'anime'), (4, 'Music', 'music', '/music', NULL);
         INSERT INTO quality_profiles (id, source, name, upgrade_allowed, cutoff, min_format_score,
                 cutoff_format_score, min_upgrade_format_score, updated_at)
             VALUES (1, 'radarr', 'HD-1080p', 1, 'Bluray-1080p', 0, 0, 1, 0);
         INSERT INTO tmdb_movies (tmdb_id, title, release_date, year, poster_path, fetched_at) VALUES
             (10, 'On Disk', '2001-01-01', 2001, '/p10.jpg', 0),
             (11, 'Coming', date('now', '+1 year'), NULL, NULL, 0),
             (12, 'Out', '1999-05-05', 1999, NULL, 0),
             (13, 'Announced', NULL, NULL, NULL, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

async fn file(pool: &SqlitePool, library: i64, path: &str, size: i64, movie: Option<i64>) {
    sqlx::query(
        "INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation, added_at) VALUES (?, ?, ?, 0, ?, 1, 1, 100)",
    )
    .bind(library)
    .bind(path)
    .bind(size)
    .bind(movie)
    .execute(pool)
    .await
    .unwrap();
}

fn by_title<D>(titles: &[Title<D>], name: &str) -> (State, i64, bool) {
    let t = titles
        .iter()
        .find(|t| t.title == name)
        .unwrap_or_else(|| panic!("no {name}"));
    (t.state, t.size, t.wanted)
}

#[tokio::test]
async fn movies_join_the_library_and_the_wanted_list() {
    let pool = fixture().await;
    sqlx::raw_sql(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation,
                 match_tmdb_id, match_status) VALUES
             (1, 1, 'On Disk (2001)', 'On Disk', 2001, 1, 10, 'matched'),
             (2, 1, 'Second Copy', 'On Disk', 2001, 1, 10, 'matched'),
             (3, 1, 'Nobody Wants (1980)', 'Nobody Wants', 1980, 1, NULL, 'unmatched'),
             (4, 1, 'Empty (1990)', 'Empty', 1990, 1, NULL, 'unmatched');
         INSERT INTO wanted_movies (id, tmdb_id, title, year, monitored, profile_id, library_id,
                 movie_id, source, added_at) VALUES
             (1, 10, 'On Disk', 2001, 1, 1, 1, 1, 'radarr', 5),
             (2, 11, 'Coming', NULL, 1, NULL, 1, NULL, 'manual', 6),
             (3, 12, 'Out', 1999, 1, NULL, 1, NULL, 'manual', 7),
             (4, 13, 'Announced', NULL, 1, NULL, 1, NULL, 'manual', 8),
             (5, 14, 'Not Looked For', NULL, 0, NULL, 1, NULL, 'manual', 9),
             (6, 15, 'On Its Way', NULL, 1, NULL, 1, NULL, 'manual', 10),
             (7, 16, 'Unknown To TMDB', NULL, 1, NULL, 1, NULL, 'manual', 11),
             (8, 17, 'Just Imported', NULL, 1, NULL, 1, NULL, 'manual', 12);
         INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, progress,
                 added_at, updated_at) VALUES
             ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'On.Its.Way.1080p', 6, '{}', 'downloading', 40, 0, 0),
             ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'On.Disk.2160p', 1, '{}', 'downloading', 10, 0, 0),
             ('cccccccccccccccccccccccccccccccccccccccc', 'Just.Imported', 8, '{}', 'imported', 100, 0, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    file(
        &pool,
        1,
        "On Disk (2001)/On.Disk.2001.1080p.BluRay.x264.mkv",
        3,
        Some(1),
    )
    .await;
    file(
        &pool,
        1,
        "Second Copy/On.Disk.2001.720p.HDTV.mkv",
        2,
        Some(2),
    )
    .await;
    file(
        &pool,
        1,
        "Nobody Wants (1980)/Nobody.Wants.1980.DVDRip.avi",
        1,
        Some(3),
    )
    .await;

    let mut titles = movies(&pool).await.unwrap();
    assert_eq!(
        titles.len(),
        9,
        "one row per title; the empty folder isn't one"
    );
    assert!(
        titles.iter().all(|t| t.detail.quality.is_none()),
        "read for a page only"
    );
    qualities(&pool, &mut titles).await.unwrap();
    // Both folders matched to its TMDB movie are the one wanted movie's.
    assert_eq!(by_title(&titles, "On Disk"), (State::InLibrary, 5, true));
    assert_eq!(by_title(&titles, "Coming"), (State::Unreleased, 0, true));
    assert_eq!(by_title(&titles, "Out"), (State::Missing, 0, true));
    assert_eq!(by_title(&titles, "Announced"), (State::Unreleased, 0, true));
    assert_eq!(
        by_title(&titles, "Not Looked For"),
        (State::Unmonitored, 0, true)
    );
    assert_eq!(
        by_title(&titles, "On Its Way"),
        (State::Downloading, 0, true)
    );
    assert_eq!(
        by_title(&titles, "Unknown To TMDB"),
        (State::Missing, 0, true)
    );
    assert_eq!(
        by_title(&titles, "Just Imported"),
        (State::InLibrary, 0, true)
    );
    assert_eq!(
        by_title(&titles, "Nobody Wants"),
        (State::InLibrary, 1, false)
    );

    let on_disk = titles.iter().find(|t| t.title == "On Disk").unwrap();
    assert_eq!(on_disk.detail.movie_id, Some(1));
    assert_eq!(
        on_disk.detail.quality.as_deref(),
        Some("Bluray-1080p"),
        "its largest file"
    );
    assert_eq!(on_disk.profile.as_deref(), Some("HD-1080p"));
    assert_eq!(on_disk.library.as_deref(), Some("Films"));
    assert_eq!(
        on_disk.poster.as_deref(),
        Some("/api/v1/artwork/w342/p10.jpg")
    );
    assert_eq!(
        on_disk.download.as_ref().map(|d| (d.step, d.percent)),
        Some((Step::Downloading, Some(10.0))),
        "the upgrade on its way is shown, and it stays in the library"
    );
    let nobody = titles.iter().find(|t| t.title == "Nobody Wants").unwrap();
    assert_eq!(nobody.detail.quality.as_deref(), Some("DVD"));
    assert_eq!((nobody.monitored, nobody.added_at), (false, Some(100)));
}

async fn episode(pool: &SqlitePool, library: i64, series: i64, s: i64, e: i64, end: Option<i64>) {
    sqlx::query(
        "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
             episode, episode_end, first_seen_generation, seen_generation)
         VALUES (?, ?, 10, 0, ?, ?, ?, ?, 1, 1)",
    )
    .bind(library)
    .bind(format!("{series}/S{s:02}E{e:02}.mkv"))
    .bind(series)
    .bind(s)
    .bind(e)
    .bind(end)
    .execute(pool)
    .await
    .unwrap();
}

/// Series of every state, anime among them.
async fn series_fixture() -> SqlitePool {
    let pool = fixture().await;
    sqlx::raw_sql(
        "INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES
             (100, 'Complete Show', 0), (101, 'Partial Show', 0), (102, 'Future Show', 0);
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, air_date) VALUES
             (100, 1, 1, 1, '2001-01-01'), (100, 1, 2, 2, '2001-01-08'),
             (101, 1, 1, 3, '2002-01-01'), (101, 1, 2, 4, '2002-01-08'),
             (101, 2, 1, 5, '2003-01-01'), (101, 2, 2, 6, date('now', '+1 month')),
             (102, 1, 1, 7, date('now', '+1 month'));
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id) VALUES
             (1, 2, 'Complete Show', 'Complete Show', 1, 100),
             (2, 2, 'Partial Show', 'Partial Show', 1, 101),
             (5, 3, 'Some Anime', 'Some Anime', 1, NULL),
             (7, 2, 'Plain Show', 'Plain Show', 1, NULL);
         INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type, library_id,
                 series_id, source, added_at) VALUES
             (1, 1000, 100, 'Complete Show', 1, 'standard', 2, 1, 'sonarr', 0),
             (2, 1001, 101, 'Partial Show', 1, 'standard', 2, NULL, 'sonarr', 0),
             (3, 1002, 102, 'Future Show', 1, 'standard', 2, NULL, 'manual', 0),
             (4, 1003, NULL, 'Anime Wanted', 1, 'anime', 2, NULL, 'manual', 0),
             (6, 1005, NULL, 'Off', 0, 'standard', 2, NULL, 'manual', 0);
         INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES
             (1, 1, 1), (2, 1, 1), (2, 2, 1), (3, 1, 1), (4, 1, 1), (6, 1, 1);
         INSERT INTO wanted_episode_overrides (wanted_series_id, season, episode, monitored)
             VALUES (2, 1, 2, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    episode(&pool, 2, 1, 1, 1, Some(2)).await;
    episode(&pool, 2, 2, 1, 1, None).await;
    episode(&pool, 3, 5, 1, 1, None).await;
    episode(&pool, 2, 7, 1, 1, None).await;
    pool
}

fn episodes(titles: &[Title<SeriesDetail>], name: &str) -> (i64, Option<i64>) {
    let t = titles.iter().find(|t| t.title == name).unwrap();
    (t.detail.episodes_on_disk, t.detail.episodes_monitored)
}

#[tokio::test]
async fn series_count_monitored_episodes_and_anime_is_apart() {
    let pool = series_fixture().await;
    let shows = series(&pool, false).await.unwrap();
    let mut names: Vec<&str> = shows.iter().map(|t| t.title.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "Complete Show",
            "Future Show",
            "Off",
            "Partial Show",
            "Plain Show"
        ]
    );
    // One file holding two episodes counts for both.
    assert_eq!(
        by_title(&shows, "Complete Show"),
        (State::InLibrary, 10, true)
    );
    assert_eq!(episodes(&shows, "Complete Show"), (2, Some(2)));
    // Claimed through its TMDB match; the episode left out and the one to come don't count.
    assert_eq!(by_title(&shows, "Partial Show"), (State::Missing, 10, true));
    assert_eq!(episodes(&shows, "Partial Show"), (1, Some(2)));
    assert_eq!(by_title(&shows, "Future Show").0, State::Unreleased);
    assert_eq!(by_title(&shows, "Off").0, State::Unmonitored);
    assert_eq!(
        by_title(&shows, "Plain Show"),
        (State::InLibrary, 10, false)
    );

    let anime = series(&pool, true).await.unwrap();
    let mut names: Vec<&str> = anime.iter().map(|t| t.title.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["Anime Wanted", "Some Anime"],
        "wanted as anime, or in an anime library"
    );
    // TMDB's list isn't here: a monitored season with nothing on disk is missing.
    assert_eq!(by_title(&anime, "Anime Wanted").0, State::Missing);
    assert_eq!(episodes(&anime, "Anime Wanted"), (0, None));
    assert_eq!(
        by_title(&anime, "Some Anime"),
        (State::InLibrary, 10, false)
    );
}

#[tokio::test]
async fn a_series_downloads_only_what_it_lacks() {
    let pool = series_fixture().await;
    let state = || async {
        let shows = series(&pool, false).await.unwrap();
        by_title(&shows, "Partial Show").0
    };
    // An upgrade of season 1, which has nothing missing.
    sqlx::query(
        "INSERT INTO downloads (infohash, title, wanted_series_id, season, decision, state,
             added_at, updated_at)
         VALUES ('dddddddddddddddddddddddddddddddddddddddd', 'Partial.Show.S01', 2, 1, '{}',
                 'downloading', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(state().await, State::Missing);
    sqlx::query(
        "INSERT INTO downloads (infohash, title, wanted_series_id, season, decision, state,
             added_at, updated_at)
         VALUES ('eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee', 'Partial.Show.S02', 2, 2, '{}',
                 'transferring', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(state().await, State::Downloading);
}

#[tokio::test]
async fn artists_count_wanted_albums_on_disk() {
    let pool = fixture().await;
    sqlx::raw_sql(
        "INSERT INTO music_artists (id, key, name, mb_artist_id) VALUES
             (1, 'radiohead', 'Radiohead', 'mb-rh'), (2, 'bjork', 'Björk', 'mb-bj');
         INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
                 mb_release_group_id, seen_generation) VALUES
             (1, 4, 'mb:1', 'OK Computer', 1, 'Radiohead', 'Radiohead/OK Computer', 'rg-ok', 1),
             (2, 4, 'mb:2', 'Debut', 2, 'Björk', 'Björk/Debut', 'rg-debut', 1);
         INSERT INTO music_tracks (library_id, rel_path, size, modified_at, first_seen_generation,
                 seen_generation, album_id, tagged, title) VALUES
             (4, 'Radiohead/OK Computer/01.flac', 30, 50, 1, 1, 1, 1, 'Airbag'),
             (4, 'Björk/Debut/01.flac', 20, 60, 1, 1, 2, 1, 'Human Behaviour');
         INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, library_id,
                 source, added_at) VALUES
             (1, 'mb-rh', 'Radiohead', 1, 'all', 4, 'lidarr', 0),
             (2, 'mb-new', 'Someone New', 1, 'new', 4, 'manual', 0),
             (3, 'mb-off', 'Not Followed', 0, 'none', 4, 'manual', 0);
         INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, release_date,
                 monitored, source, added_at) VALUES
             (1, 1, 'rg-ok', 'OK Computer', '1997-05-21', 1, 'lidarr', 0),
             (2, 1, 'rg-kid', 'Kid A', '2000', 1, 'lidarr', 0),
             (3, 1, 'rg-live', 'Live', '2001', 0, 'lidarr', 0),
             (4, 2, 'rg-soon', 'Soon', '2999-01', 1, 'manual', 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let titles = artists(&pool).await.unwrap();
    assert_eq!(titles.len(), 4);
    assert_eq!(by_title(&titles, "Radiohead"), (State::Missing, 30, true));
    let radiohead = titles.iter().find(|t| t.title == "Radiohead").unwrap();
    assert_eq!(
        (
            radiohead.detail.albums_on_disk,
            radiohead.detail.albums_wanted
        ),
        (1, 2),
        "the album left unmonitored doesn't count"
    );
    assert_eq!(radiohead.detail.artist_id, Some(1));
    assert_eq!(by_title(&titles, "Someone New").0, State::Unreleased);
    assert_eq!(by_title(&titles, "Not Followed").0, State::Unmonitored);
    assert_eq!(by_title(&titles, "Björk"), (State::InLibrary, 20, false));

    sqlx::query(
        "INSERT INTO downloads (source, soulseek, title, wanted_album_id, decision, state,
             progress, added_at, updated_at)
         VALUES ('soulseek', '{\"username\":\"a\",\"folder\":\"f\",\"parts\":[]}', 'Kid A', 2,
                 '{}', 'downloading', 5, 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let titles = artists(&pool).await.unwrap();
    assert_eq!(by_title(&titles, "Radiohead").0, State::Downloading);
}

/// The fixture with the captured Radarr and Sonarr profiles saved.
async fn profiled() -> SqlitePool {
    let pool = fixture().await;
    for app in [crate::profiles::App::Radarr, crate::profiles::App::Sonarr] {
        crate::profiles::save(&pool, app, &crate::profiles::import::tests::converted(app))
            .await
            .unwrap();
    }
    pool
}

fn cutoff_of<D>(titles: &[Title<D>], name: &str) -> Option<bool> {
    titles
        .iter()
        .find(|t| t.title == name)
        .unwrap()
        .cutoff_unmet
}

#[tokio::test]
async fn cutoff_unmet_is_the_upgrades_own_decision() {
    let pool = profiled().await;
    // 4K Preferred upgrades to Remux-2160p and an upgrade-until score of 0; HD-1080p doesn't
    // upgrade at all.
    sqlx::raw_sql(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation, tmdb_id)
             SELECT value, 1, 'Movie ' || value, 'Movie ' || value, 1, 100 + value
             FROM json_each('[1, 2, 3, 4, 5, 6, 7]');
         INSERT INTO wanted_movies (id, tmdb_id, title, monitored, quality_profile, movie_id,
                 source, added_at) VALUES
             (1, 101, 'Below', 1, '4K Preferred', 1, 'radarr', 0),
             (2, 102, 'At Cutoff', 1, '4K Preferred', 2, 'radarr', 0),
             (3, 103, 'Scored Low', 1, '4K Preferred', 3, 'radarr', 0),
             (4, 104, 'Never Upgraded', 1, 'HD-1080p', 4, 'radarr', 0),
             (5, 105, 'Not Monitored', 0, '4K Preferred', 5, 'radarr', 0),
             (6, 106, 'Not Linked', 1, '4K Preferred', NULL, 'radarr', 0),
             (7, 107, 'Unreadable', 1, '4K Preferred', 7, 'radarr', 0),
             (8, 108, 'No Profile', 1, 'Gone', NULL, 'radarr', 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (movie, name) in [
        (1, "Below.2020.1080p.BluRay.x264.mkv"),
        (
            2,
            "At.Cutoff.2020.2160p.UHD.BluRay.REMUX.DV.HDR10.TrueHD.7.1.mkv",
        ),
        (3, "Scored.Low.2020.2160p.UHD.BluRay.REMUX.VOSTFR.mkv"),
        (4, "Never.Upgraded.2020.720p.HDTV.x264.mkv"),
        (5, "Not.Monitored.2020.720p.HDTV.x264.mkv"),
        (6, "Not.Linked.2020.720p.HDTV.x264.mkv"),
        (7, "Something.mkv"),
    ] {
        file(&pool, 1, &format!("Movie {movie}/{name}"), 1, Some(movie)).await;
    }
    let titles = movies(&pool).await.unwrap();
    let expected = [
        ("Below", Some(true)),
        ("At Cutoff", Some(false)),
        ("Scored Low", Some(true)),
        ("Never Upgraded", Some(false)),
        ("Not Monitored", None),
        ("Not Linked", None),
        ("Unreadable", None),
        ("No Profile", None),
    ];
    for (name, unmet) in expected {
        assert_eq!(cutoff_of(&titles, name), unmet, "{name}");
    }
    // The same answer as an upgrade search for each monitored one would give.
    for id in [1, 2, 3, 4, 6, 7, 8] {
        let name = titles.iter().find(|t| t.wanted_id == Some(id)).unwrap();
        let searched = crate::search::cutoff_unmet(&pool, crate::search::Target::Movie(id))
            .await
            .unwrap();
        assert_eq!(name.cutoff_unmet, searched, "{}", name.title);
    }
    let all = page(titles, &Filter::default(), 100, 0);
    assert_eq!(all.counts.cutoff_unmet, 2);
    let below = Filter {
        cutoff_unmet: Some(true),
        ..Filter::default()
    };
    let titles = movies(&pool).await.unwrap();
    assert_eq!(
        names(&page(titles, &below, 100, 0)),
        ["Below", "Scored Low"]
    );
}

#[tokio::test]
async fn a_series_is_below_its_cutoff_when_a_monitored_episode_is() {
    let pool = profiled().await;
    sqlx::raw_sql(
        "INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (9, 2, 'Show', 'Show', 1);
         INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, quality_profile,
                 library_id, series_id, source, added_at)
             VALUES (9, 9000, 'Show', 1, 'standard', '4K Preferred', 2, 9, 'sonarr', 0);
         INSERT INTO wanted_seasons (wanted_series_id, season, monitored)
             VALUES (9, 1, 1), (9, 2, 0);
         INSERT INTO wanted_episode_overrides (wanted_series_id, season, episode, monitored)
             VALUES (9, 1, 3, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Episode 1 is below the cutoff, episode 2 at it; episode 3 is left out on its own, and
    // season 2 isn't monitored.
    for (s, e, name) in [
        (1, 1, "Show.S01E01.1080p.WEB-DL.mkv"),
        (
            1,
            2,
            "Show.S01E02.2160p.BluRay.REMUX.DV.HDR10.TrueHD.7.1.mkv",
        ),
        (1, 3, "Show.S01E03.720p.HDTV.mkv"),
        (2, 1, "Show.S02E01.720p.HDTV.mkv"),
    ] {
        sqlx::query(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
                 episode, first_seen_generation, seen_generation) VALUES (2, ?, 1, 0, 9, ?, ?, 1, 1)",
        )
        .bind(format!("Show/{name}"))
        .bind(s)
        .bind(e)
        .execute(&pool)
        .await
        .unwrap();
    }
    let titles = series(&pool, false).await.unwrap();
    let show = titles.iter().find(|t| t.title == "Show").unwrap();
    assert_eq!(show.cutoff_unmet, Some(true));
    assert_eq!(show.detail.unmet_episodes, [(1, 1)]);
    for (episode, unmet) in [(1, Some(true)), (2, Some(false))] {
        let target = crate::search::Target::Series {
            id: 9,
            season: 1,
            episode: Some(episode),
        };
        let searched = crate::search::cutoff_unmet(&pool, target).await.unwrap();
        assert_eq!(searched, unmet, "episode {episode}");
    }
}
