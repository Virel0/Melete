use super::*;
use crate::{auth, db};

async fn user(pool: &SqlitePool) -> User {
    auth::create_user(pool, "alice", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap()
}

use super::totp::tests::from_base32 as decode;

async fn turned_on(pool: &SqlitePool, user: &User, now: i64) -> Vec<u8> {
    let setup = start_setup(pool, user).await.unwrap().unwrap();
    let secret = decode(&setup.secret);
    let code = totp::code_at(&secret, totp::step_at(now));
    assert_eq!(
        confirm(pool, user.id, &code, now).await.unwrap(),
        Confirmed::Done
    );
    secret
}

#[tokio::test]
async fn setup_gives_a_secret_and_link_until_a_first_code_confirms_it() {
    let pool = db::open_in_memory().await;
    let user = user(&pool).await;
    let now = unix_now();
    assert_eq!(
        confirm(&pool, user.id, "123456", now).await.unwrap(),
        Confirmed::NothingWaiting
    );
    let first = start_setup(&pool, &user).await.unwrap().unwrap();
    assert_eq!(first.secret.len(), 32, "160 bits in base32");
    assert_eq!(
        first.uri,
        format!(
            "otpauth://totp/Melete:alice?secret={}&issuer=Melete&algorithm=SHA1&digits=6&period=30",
            first.secret
        )
    );
    // Starting again replaces the secret still waiting, so the first one's codes don't count.
    let second = start_setup(&pool, &user).await.unwrap().unwrap();
    assert_ne!(first.secret, second.secret);
    let old_code = totp::code_at(&decode(&first.secret), totp::step_at(now));
    assert_eq!(
        confirm(&pool, user.id, &old_code, now).await.unwrap(),
        Confirmed::WrongCode
    );
    assert!(!is_on(&pool, user.id).await.unwrap(), "waiting isn't on");

    let code = totp::code_at(&decode(&second.secret), totp::step_at(now));
    assert_eq!(
        confirm(&pool, user.id, &code, now).await.unwrap(),
        Confirmed::Done
    );
    assert_eq!(
        status(&pool, user.id).await.unwrap(),
        Status {
            on: true,
            on_since: Some(now)
        }
    );
    assert!(
        start_setup(&pool, &user).await.unwrap().is_none(),
        "no new secret while it's on"
    );
    assert_eq!(
        confirm(&pool, user.id, &code, now).await.unwrap(),
        Confirmed::AlreadyOn
    );
    assert!(
        !take_code(&pool, user.id, &code, now).await.unwrap(),
        "the code that confirmed it can't sign in too"
    );
}

#[tokio::test]
async fn a_code_is_taken_once_and_never_for_an_older_step() {
    let pool = db::open_in_memory().await;
    let user = user(&pool).await;
    let now = 1_800_000_000;
    let secret = turned_on(&pool, &user, now - 300).await;
    let step = totp::step_at(now);
    let code = totp::code_at(&secret, step);
    assert!(take_code(&pool, user.id, &code, now).await.unwrap());
    assert!(
        !take_code(&pool, user.id, &code, now).await.unwrap(),
        "reused"
    );
    let previous = totp::code_at(&secret, step - 1);
    assert!(
        !take_code(&pool, user.id, &previous, now).await.unwrap(),
        "an older step after a newer one"
    );
    let next = totp::code_at(&secret, step + 1);
    assert!(take_code(&pool, user.id, &next, now + 30).await.unwrap());
    assert!(!take_code(&pool, user.id, "000000", now + 60).await.unwrap());
}

#[tokio::test]
async fn removing_it_is_recorded_only_when_it_was_on() {
    let pool = db::open_in_memory().await;
    let user = user(&pool).await;
    start_setup(&pool, &user).await.unwrap().unwrap();
    assert!(!remove(&pool, user.id, MadeFrom::Account).await.unwrap());
    assert!(changes(&pool, user.id).await.unwrap().is_empty());

    turned_on(&pool, &user, unix_now()).await;
    start_challenge(&pool, user.id, "phone", false)
        .await
        .unwrap();
    assert!(remove(&pool, user.id, MadeFrom::Server).await.unwrap());
    assert!(!is_on(&pool, user.id).await.unwrap());
    let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM sign_in_challenges")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(waiting, 0, "sign-ins waiting for a code go with it");
    let made: Vec<(String, String)> = changes(&pool, user.id)
        .await
        .unwrap()
        .into_iter()
        .map(|c| (c.change, c.made_from))
        .collect();
    assert_eq!(
        made,
        [
            ("two_factor_off".into(), "server".into()),
            ("two_factor_on".into(), "account".into())
        ]
    );
}

#[tokio::test]
async fn a_challenge_finishes_once_the_way_it_started_and_runs_out() {
    let pool = db::open_in_memory().await;
    let user = user(&pool).await;
    let now = unix_now();
    let challenge = start_challenge(&pool, user.id, "phone", false)
        .await
        .unwrap();
    assert!(challenge.starts_with(CHALLENGE_PREFIX));
    let stored: Vec<u8> = sqlx::query_scalar("SELECT challenge_hash FROM sign_in_challenges")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(stored, challenge.as_bytes(), "only its hash is kept");
    assert_eq!(
        challenged(&pool, &challenge, true, now).await.unwrap(),
        None,
        "an app's challenge doesn't finish to a cookie"
    );
    assert_eq!(
        challenged(&pool, &challenge, false, now).await.unwrap(),
        Some(user.id)
    );
    let late = now + CHALLENGE_LIFETIME + 1;
    assert_eq!(
        challenged(&pool, &challenge, false, late).await.unwrap(),
        None
    );
    assert_eq!(
        finish_challenge(&pool, &challenge, late).await.unwrap(),
        None
    );
    assert_eq!(
        finish_challenge(&pool, &challenge, now)
            .await
            .unwrap()
            .as_deref(),
        Some("phone")
    );
    assert_eq!(
        finish_challenge(&pool, &challenge, now).await.unwrap(),
        None
    );
    assert_eq!(
        challenged(&pool, &challenge, false, now).await.unwrap(),
        None
    );
    assert_eq!(
        challenged(&pool, "mlt_not-a-challenge", false, now)
            .await
            .unwrap(),
        None
    );
}
