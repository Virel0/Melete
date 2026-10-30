use super::*;
use crate::db;

fn went(report: RefreshReport) -> anyhow::Result<RefreshReport> {
    Ok(report)
}

#[test]
fn a_refused_token_is_a_problem_at_once() {
    let mut status = Status::default();
    assert_eq!(finding(&status), None);
    status.record(&Err(
        anyhow::Error::new(Unauthorized).context("matching Dune")
    ));
    assert_eq!(status.trouble, Some(Trouble::TokenRefused));
    let found = finding(&status).unwrap();
    assert_eq!((found.level, found.code), (Level::Problem, "tmdb_refused"));
    assert!(
        found.message.contains("Outside services"),
        "{}",
        found.message
    );

    // A run that goes through clears it.
    status.record(&went(RefreshReport {
        matched: 1,
        ..RefreshReport::default()
    }));
    assert_eq!(status, Status::default());
    assert_eq!(finding(&status), None);
}

#[test]
fn runs_that_get_nothing_from_tmdb_are_a_problem_from_the_second() {
    let mut status = Status::default();
    let nothing = || {
        went(RefreshReport {
            errors: 4,
            ..RefreshReport::default()
        })
    };
    status.record(&nothing());
    assert_eq!(status.trouble, Some(Trouble::NothingWentThrough));
    assert_eq!(finding(&status), None, "one run can be a blip");
    status.record(&Err(anyhow::anyhow!("the database is locked")));
    let found = finding(&status).unwrap();
    assert_eq!(
        (found.level, found.code, found.count),
        (Level::Problem, "tmdb_failing", 2)
    );
    assert!(found.message.contains("last 2 runs"), "{}", found.message);

    // Some titles failing while others go through isn't TMDB failing, and starts the count again.
    status.record(&went(RefreshReport {
        unmatched: 1,
        errors: 3,
        ..RefreshReport::default()
    }));
    assert_eq!(
        status,
        Status {
            trouble: Some(Trouble::SomeFailed),
            failing_runs: 0
        }
    );
    assert_eq!(finding(&status), None);
    status.record(&Err(RateLimited(std::time::Duration::from_secs(90)).into()));
    assert_eq!(status.trouble, Some(Trouble::RateLimited));
    assert_eq!(status.failing_runs, 1);
}

#[tokio::test]
async fn the_task_says_what_titles_stand_at_and_what_went_wrong_without_the_error() {
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'films', 'movies', '/m'),
             (2, 'shows', 'shows', '/s');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_method)
         VALUES (1, 1, 'A', 'A', 0, 'matched', 'search'), (2, 1, 'B', 'B', 0, 'unmatched', 'search'),
             (3, 1, 'C', 'C', 0, 'unmatched', 'manual'), (4, 1, 'D', 'D', 0, NULL, NULL);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_method)
         VALUES (1, 2, 'E', 'E', 0, 'ambiguous', 'search'), (2, 2, 'F', 'F', 0, 'matched', 'manual');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let ok = went(RefreshReport {
        matched: 1,
        errors: 2,
        ..RefreshReport::default()
    });
    assert_eq!(
        summary(&pool, &ok).await,
        "this run matched 1; 2 failed and are tried again next run. In all, 2 matched, 2 need a match"
    );
    let none = went(RefreshReport {
        errors: 2,
        ..RefreshReport::default()
    });
    assert_eq!(
        summary(&pool, &none).await,
        "failed: no request to TMDB went through, for 2 titles; the server's log says why. In all, 2 matched, 2 need a match"
    );
    let refused = Err(anyhow::Error::new(Unauthorized).context("https://tmdb.example/3/search"));
    let said = summary(&pool, &refused).await;
    assert_eq!(said, "failed: TMDB refused the token");
    let stopped = Err(anyhow::anyhow!(
        "connecting to https://tmdb.example: refused"
    ));
    let said = summary(&pool, &stopped).await;
    assert!(!said.contains("tmdb.example"), "{said}");
    assert!(said.starts_with("failed"), "{said}");
}
