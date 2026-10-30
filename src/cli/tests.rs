use super::*;
use crate::db;
use crate::two_factor::totp;

/// Alice, with her second factor on.
async fn alice_with_a_second_factor(pool: &sqlx::SqlitePool) -> i64 {
    let user = auth::create_user(pool, "alice", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let setup = two_factor::start_setup(pool, &user).await.unwrap().unwrap();
    let secret = totp::tests::from_base32(&setup.secret);
    let now = crate::db::unix_now();
    let code = totp::code_at(&secret, totp::step_at(now));
    two_factor::confirm(pool, user.id, &code, now)
        .await
        .unwrap();
    user.id
}

fn never_asked(_: &str) -> anyhow::Result<bool> {
    panic!("asked when it shouldn't be")
}

#[tokio::test]
async fn a_reset_removes_the_second_factor_when_the_answer_is_yes() {
    let pool = db::open_in_memory().await;
    let id = alice_with_a_second_factor(&pool).await;
    let mut asked_about = None;
    reset_password(&pool, "alice", SecondFactor::Ask, |name| {
        asked_about = Some(name.to_owned());
        Ok(true)
    })
    .await
    .unwrap();
    assert_eq!(asked_about.as_deref(), Some("alice"));
    assert!(!two_factor::is_on(&pool, id).await.unwrap());
    let changes = two_factor::changes(&pool, id).await.unwrap();
    assert_eq!(
        (changes[0].change.as_str(), changes[0].made_from.as_str()),
        ("two_factor_off", "server")
    );
    assert!(
        auth::check_password(&pool, "alice", "a-long-password".into())
            .await
            .unwrap()
            .is_none(),
        "the password was reset"
    );
}

#[tokio::test]
async fn a_reset_keeps_the_second_factor_when_the_answer_is_no_or_the_flag_says_so() {
    let pool = db::open_in_memory().await;
    let id = alice_with_a_second_factor(&pool).await;
    reset_password(&pool, "alice", SecondFactor::Ask, |_| Ok(false))
        .await
        .unwrap();
    assert!(two_factor::is_on(&pool, id).await.unwrap());
    reset_password(&pool, "alice", SecondFactor::Keep, never_asked)
        .await
        .unwrap();
    assert!(two_factor::is_on(&pool, id).await.unwrap());
    reset_password(&pool, "alice", SecondFactor::Remove, never_asked)
        .await
        .unwrap();
    assert!(!two_factor::is_on(&pool, id).await.unwrap());
}

#[tokio::test]
async fn a_reset_asks_nothing_of_a_user_without_one() {
    let pool = db::open_in_memory().await;
    auth::create_user(&pool, "bob", "a-long-password".into(), false)
        .await
        .unwrap();
    reset_password(&pool, "bob", SecondFactor::Ask, never_asked)
        .await
        .unwrap();
    assert!(
        reset_password(&pool, "nobody", SecondFactor::Ask, never_asked)
            .await
            .is_err()
    );
}

#[test]
fn reset_takes_one_second_factor_flag_at_most() {
    assert_eq!(SecondFactor::from_flags(&[]), Some(SecondFactor::Ask));
    assert_eq!(
        SecondFactor::from_flags(&["--remove-second-factor"]),
        Some(SecondFactor::Remove)
    );
    assert_eq!(
        SecondFactor::from_flags(&["--keep-second-factor"]),
        Some(SecondFactor::Keep)
    );
    for bad in [
        &["--admin"][..],
        &["--remove-second-factor", "--keep-second-factor"],
    ] {
        assert_eq!(SecondFactor::from_flags(bad), None);
    }
}
