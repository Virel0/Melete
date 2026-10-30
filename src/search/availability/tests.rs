use super::*;
use crate::rss::tests::{indexer, releases, setup, want_dune};

/// Dates as they'd be on 2026-06-01 (out by the 2nd).
fn dates(cinema: Option<&str>, digital: Option<&str>, physical: Option<&str>) -> Dates {
    Dates {
        cinema: cinema.map(Into::into),
        digital: digital.map(Into::into),
        physical: physical.map(Into::into),
        tv: None,
        cinema_plus_90: cinema.map(|c| match c {
            "2026-05-01" => "2026-07-30".into(),
            "2025-01-01" => "2025-04-01".into(),
            other => panic!("no +90 for {other}"),
        }),
        out_by: "2026-06-02".into(),
    }
}

#[test]
fn a_film_in_cinemas_gets_cinema_sources_only_and_only_when_the_profile_allows() {
    let showing = dates(Some("2026-05-01"), Some("2026-07-15"), None);
    let web = Some(Source::WebRip);
    let cam = Some(Source::Cam);
    assert_eq!(
        refusal(Minimum::Released, &showing, web).as_deref(),
        Some("the film isn't out on TV, digitally or on disc until 2026-07-15")
    );
    assert_eq!(
        refusal(Minimum::InCinemas, &showing, web).as_deref(),
        Some(
            "a WEB or TV release before the film's digital or TV release on 2026-07-15 is most likely a fake"
        )
    );
    assert_eq!(refusal(Minimum::InCinemas, &showing, cam), None);
    assert_eq!(
        refusal(Minimum::Announced, &showing, Some(Source::Bluray)),
        None,
        "no disc date: no fake is called on a stand-in"
    );
}

#[test]
fn a_film_not_yet_in_cinemas_gets_nothing_but_with_announced() {
    let coming = Dates {
        cinema: Some("2026-12-18".into()),
        cinema_plus_90: Some("2027-03-18".into()),
        out_by: "2026-06-02".into(),
        ..Dates::default()
    };
    assert_eq!(
        refusal(Minimum::InCinemas, &coming, Some(Source::Cam)).as_deref(),
        Some("the film isn't in cinemas until 2026-12-18")
    );
    assert!(
        refusal(Minimum::Announced, &coming, Some(Source::Telesync))
            .unwrap()
            .starts_with("a cinema recording before")
    );
    assert_eq!(refusal(Minimum::Announced, &coming, None), None);
    // Its digital date isn't known, so a WEB release isn't called a fake: only the minimum holds
    // it back, and "announced" doesn't.
    assert_eq!(
        refusal(Minimum::Announced, &coming, Some(Source::WebDl)),
        None
    );
    assert!(refusal(Minimum::Released, &coming, Some(Source::WebDl)).is_some());
}

#[test]
fn a_tv_films_premiere_counts_like_a_digital_release() {
    // A TV film: no cinema release, first shown on TV last month, no digital date.
    let tv_film = Dates {
        tv: Some("2026-05-10".into()),
        out_by: "2026-06-02".into(),
        ..Dates::default()
    };
    for source in [Source::Hdtv, Source::WebDl, Source::WebRip] {
        for minimum in [Minimum::Announced, Minimum::InCinemas, Minimum::Released] {
            assert_eq!(
                refusal(minimum, &tv_film, Some(source)),
                None,
                "{source:?} {minimum:?}"
            );
        }
    }
    let premieres_later = Dates {
        tv: Some("2026-08-01".into()),
        ..tv_film
    };
    assert_eq!(
        refusal(Minimum::Announced, &premieres_later, Some(Source::Hdtv)).as_deref(),
        Some(
            "a WEB or TV release before the film's digital or TV release on 2026-08-01 is most likely a fake"
        )
    );
}

#[test]
fn a_released_film_is_taken_with_a_days_slack_and_undated_films_are_not_judged() {
    let out = dates(Some("2025-01-01"), Some("2026-06-02"), Some("2026-06-30"));
    assert_eq!(refusal(Minimum::Released, &out, Some(Source::WebDl)), None);
    assert!(refusal(Minimum::Released, &out, Some(Source::Bluray)).is_some());
    // No digital or disc date: 90 days after the cinema release, for the minimum only.
    let old = dates(Some("2025-01-01"), None, None);
    assert_eq!(refusal(Minimum::Released, &old, Some(Source::Bluray)), None);
    let showing = dates(Some("2026-05-01"), None, None);
    assert_eq!(
        refusal(Minimum::Released, &showing, Some(Source::WebDl)).as_deref(),
        Some("the film isn't out on TV, digitally or on disc until 2026-07-30")
    );
    assert_eq!(
        refusal(Minimum::InCinemas, &showing, Some(Source::WebDl)),
        None
    );
    assert_eq!(
        refusal(
            Minimum::Released,
            &dates(None, None, None),
            Some(Source::WebDl)
        ),
        None
    );
}

/// Dune: Part Two in cinemas 20 days ago, out digitally `digital` days from now, its release
/// dates fetched.
async fn showing(pool: &SqlitePool, digital: i64) {
    sqlx::query(
        "INSERT INTO tmdb_movies (tmdb_id, title, release_date, digital_release, fetched_at,
             release_dates_at)
         VALUES (693134, 'Dune: Part Two', date('now', '-20 days'), date('now', ? || ' days'), 0, 0)
         ON CONFLICT (tmdb_id) DO UPDATE SET digital_release = excluded.digital_release",
    )
    .bind(format!("{digital:+}"))
    .execute(pool)
    .await
    .unwrap();
}

/// Each release of a search: its title, whether accepted, and its last reason.
async fn verdicts(pool: &SqlitePool, search_id: i64) -> Vec<(String, bool, Option<String>)> {
    let view = get(pool, search_id).await.unwrap().unwrap();
    view.releases
        .into_iter()
        .map(|r| {
            let last = r.decision["rejections"]
                .as_array()
                .and_then(|a| a.last())
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            (r.title, r.accepted, last)
        })
        .collect()
}

const WEB: &str = "Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX";

#[tokio::test]
async fn a_web_release_before_the_digital_date_is_refused_at_search_and_accepted_after() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    showing(&pool, 30).await;
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[(WEB, "")]);
    let client = torznab::Client::new().unwrap();

    let before = search(&pool, &client, None, Target::Movie(dune))
        .await
        .unwrap()
        .unwrap();
    let verdicts_before = verdicts(&pool, before).await;
    assert_eq!(verdicts_before.len(), 1);
    let (_, accepted, why) = &verdicts_before[0];
    assert!(!accepted);
    assert!(
        why.as_deref()
            .unwrap()
            .starts_with("the film isn't out on TV, digitally or on disc until "),
        "{why:?}"
    );

    // Out digitally yesterday: taken.
    showing(&pool, -1).await;
    let after = search(&pool, &client, None, Target::Movie(dune))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(verdicts(&pool, after).await, [(WEB.to_owned(), true, None)]);
}

#[tokio::test]
async fn a_web_release_before_the_digital_date_is_refused_in_rss_sync_whatever_the_minimum() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    showing(&pool, 30).await;
    // Even a profile that takes films once they're in cinemas: a WEB release can't exist yet.
    sqlx::query("UPDATE quality_profiles SET minimum_availability = 'in_cinemas'")
        .execute(&pool)
        .await
        .unwrap();
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[(WEB, "")]);
    let client = torznab::Client::new().unwrap();

    let view = crate::rss::sync(&pool, &client, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.searches.len(), 1);
    assert_eq!(view.searches[0].wanted_movie_id, Some(dune));
    let refused = verdicts(&pool, view.searches[0].search_id).await;
    assert_eq!(refused.len(), 1);
    assert!(!refused[0].1);
    let why = refused[0].2.as_deref().unwrap();
    assert!(
        why.starts_with("a WEB or TV release before the film's digital or TV release on ")
            && why.ends_with(" is most likely a fake"),
        "{why}"
    );

    // Once it's out digitally, a new WEB release in the feed is taken.
    showing(&pool, 0).await;
    let later = "Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-OTHER";
    feed.lock().unwrap().0 = releases(&[(later, "")]);
    let view = crate::rss::sync(&pool, &client, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        verdicts(&pool, view.searches[0].search_id).await,
        [(later.to_owned(), true, None)]
    );
}

#[tokio::test]
async fn announced_still_refuses_a_disc_release_before_the_film_is_out() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    sqlx::query(
        "INSERT INTO tmdb_movies (tmdb_id, title, release_date, physical_release, fetched_at,
             release_dates_at)
         VALUES (693134, 'Dune: Part Two', date('now', '+60 days'), date('now', '+150 days'), 0, 0);
         UPDATE quality_profiles SET minimum_availability = 'announced';",
    )
    .execute(&pool)
    .await
    .unwrap();
    let feed = indexer(&pool, "[2000]").await;
    let remux = "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR";
    feed.lock().unwrap().0 = releases(&[(remux, "")]);
    let client = torznab::Client::new().unwrap();
    let id = search(&pool, &client, None, Target::Movie(dune))
        .await
        .unwrap()
        .unwrap();
    let refused = verdicts(&pool, id).await;
    assert!(
        refused[0]
            .2
            .as_deref()
            .unwrap()
            .starts_with("a disc release before"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_film_whose_release_dates_arent_fetched_yet_isnt_judged() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    showing(&pool, 30).await;
    sqlx::query("UPDATE tmdb_movies SET release_dates_at = NULL")
        .execute(&pool)
        .await
        .unwrap();
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[(WEB, "")]);
    let client = torznab::Client::new().unwrap();
    let id = search(&pool, &client, None, Target::Movie(dune))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(verdicts(&pool, id).await, [(WEB.to_owned(), true, None)]);
}
