use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// Waits (up to ten seconds, as other tests may slow this one) until `done` holds.
async fn until(done: impl Fn() -> bool) {
    for _ in 0..1000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("it didn't happen in time");
}

fn following() -> TaskView {
    list().into_iter().find(|t| t.name == "following").unwrap()
}

/// The task "following", as its loop runs it: each run waits for the test to let it end, and runs
/// are counted, as are runs going at once.
#[tokio::test]
async fn running_a_task_now_never_runs_it_twice_at_once() {
    let runs = Arc::new(AtomicUsize::new(0));
    let at_once = Arc::new(AtomicUsize::new(0));
    let most_at_once = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (r, a, m, gate) = (
        runs.clone(),
        at_once.clone(),
        most_at_once.clone(),
        release.clone(),
    );
    let task = tokio::spawn(async move {
        jobs::repeat_task(
            "following",
            Duration::from_secs(3600),
            &jobs::FOLLOW,
            || {
                let (r, a, m, gate) = (r.clone(), a.clone(), m.clone(), gate.clone());
                async move {
                    let now = a.fetch_add(1, Ordering::SeqCst) + 1;
                    m.fetch_max(now, Ordering::SeqCst);
                    gate.acquire().await.expect("never closed").forget();
                    a.fetch_sub(1, Ordering::SeqCst);
                    r.fetch_add(1, Ordering::SeqCst);
                }
            },
        )
        .await
    });
    until(|| following().running).await;
    assert!(following().active);
    assert_eq!(run_now("following"), Err(NotRun::Running));
    release.add_permits(1);
    until(|| !following().running).await;
    let done = following();
    assert_eq!(done.result.as_deref(), Some("done"));
    assert!(done.next_at.unwrap() >= done.last_finished_at.unwrap() + 3599);
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    assert_eq!(run_now("following"), Ok(()));
    until(|| following().running).await;
    assert_eq!(run_now("following"), Err(NotRun::Running));
    release.add_permits(1);
    until(|| runs.load(Ordering::SeqCst) == 2 && !following().running).await;
    // Give a second run, were one wrongly asked for, the time to start.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "asking while it ran started nothing"
    );
    assert_eq!(most_at_once.load(Ordering::SeqCst), 1);
    task.abort();
}

#[test]
fn tasks_that_cant_be_run_from_here_say_so() {
    assert_eq!(run_now("nothing"), Err(NotRun::NotFound));
    assert_eq!(run_now("rss-sync"), Err(NotRun::NotByHand));
    // Its loop never started here, as when MusicBrainz is off.
    assert_eq!(run_now("music-matching"), Err(NotRun::Inactive));
    let all = list();
    assert_eq!(all.len(), TASKS.len());
    let backup = all.iter().find(|t| t.name == "backup").unwrap();
    assert!(!backup.can_run);
}

#[test]
fn a_backlog_shows_its_progress_then_how_it_went() {
    let stems = || {
        list()
            .into_iter()
            .find(|t| t.name == "stem-splitting")
            .unwrap()
    };
    // Turned off: not running in this server.
    backlog(
        "stem-splitting",
        "splitting tracks",
        false,
        (0, 10, 0),
        || unreachable!(),
    );
    assert!(!stems().active);
    // Working through it: running, with how far it has got.
    backlog(
        "stem-splitting",
        "splitting tracks",
        true,
        (3, 10, 1),
        || unreachable!(),
    );
    let now = stems();
    assert!(now.active && now.running);
    assert_eq!(
        now.progress,
        [Progress {
            what: "splitting tracks",
            done: 3,
            total: 10,
            failed: 1
        }]
    );
    let started = now.last_started_at;
    backlog(
        "stem-splitting",
        "splitting tracks",
        true,
        (4, 10, 1),
        || unreachable!(),
    );
    assert_eq!(
        stems().last_started_at,
        started,
        "one run, not one per look"
    );
    // Caught up: finished, saying how it went.
    backlog(
        "stem-splitting",
        "splitting tracks",
        true,
        (10, 10, 1),
        || "9 made".into(),
    );
    let done = stems();
    assert!(!done.running && done.progress.is_empty());
    assert_eq!(done.result.as_deref(), Some("9 made"));
    // Nothing new to do: left as it was.
    backlog(
        "stem-splitting",
        "splitting tracks",
        true,
        (10, 10, 1),
        || unreachable!(),
    );
    assert_eq!(stems().result.as_deref(), Some("9 made"));
}
