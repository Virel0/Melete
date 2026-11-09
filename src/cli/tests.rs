use super::*;
use crate::db;
use crate::two_factor::totp;

/// A user named `name` with a generated password, and their id.
async fn user(pool: &sqlx::SqlitePool, name: &str) -> (auth::User, String) {
    let password = auth::generate_password().unwrap();
    let user = auth::create_user(pool, name, password.clone(), false)
        .await
        .unwrap()
        .unwrap();
    (user, password)
}

/// Alice, with her second factor on, and her password.
async fn alice_with_a_second_factor(pool: &sqlx::SqlitePool) -> (i64, String) {
    let (user, password) = user(pool, "alice").await;
    let setup = two_factor::start_setup(pool, &user).await.unwrap().unwrap();
    let secret = totp::tests::from_base32(&setup.secret);
    let now = crate::db::unix_now();
    let code = totp::code_at(&secret, totp::step_at(now));
    two_factor::confirm(pool, user.id, &code, now)
        .await
        .unwrap();
    (user.id, password)
}

fn never_asked() -> anyhow::Result<bool> {
    panic!("asked when it shouldn't be")
}

#[tokio::test]
async fn a_reset_removes_the_second_factor_when_the_answer_is_yes() {
    let pool = db::open_in_memory().await;
    let (id, old) = alice_with_a_second_factor(&pool).await;
    let mut asked = 0;
    let (user, new, outcome) = reset_password(&pool, "alice", SecondFactor::Ask, || {
        asked += 1;
        Ok(says_yes("y\n"))
    })
    .await
    .unwrap();
    assert_eq!(asked, 1);
    assert_eq!((user.id, outcome), (id, Outcome::Removed));
    assert!(!two_factor::is_on(&pool, id).await.unwrap());
    let changes = two_factor::changes(&pool, id).await.unwrap();
    assert_eq!(
        (changes[0].change.as_str(), changes[0].made_from.as_str()),
        ("two_factor_off", "server")
    );
    // Setting it is `set_password`'s, tested with it; here the new one is a fresh generated one.
    assert_eq!(new.len(), 20);
    assert_ne!(new, old);
    assert!(
        auth::check_password(&pool, "alice", old)
            .await
            .unwrap()
            .is_none(),
        "the old password no longer signs in"
    );
}

#[tokio::test]
async fn a_reset_keeps_the_second_factor_when_the_answer_is_no_or_the_flag_says_so() {
    let pool = db::open_in_memory().await;
    let (id, _) = alice_with_a_second_factor(&pool).await;
    let (_, _, outcome) = reset_password(&pool, "alice", SecondFactor::Ask, || Ok(false))
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Kept);
    assert!(two_factor::is_on(&pool, id).await.unwrap());
    let (_, _, outcome) = reset_password(&pool, "alice", SecondFactor::Keep, never_asked)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Kept);
    assert!(two_factor::is_on(&pool, id).await.unwrap());
    let (_, _, outcome) = reset_password(&pool, "alice", SecondFactor::Remove, never_asked)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Removed);
    assert!(!two_factor::is_on(&pool, id).await.unwrap());
}

#[tokio::test]
async fn a_reset_asks_nothing_of_a_user_without_one() {
    let pool = db::open_in_memory().await;
    user(&pool, "bob").await;
    let (_, _, outcome) = reset_password(&pool, "bob", SecondFactor::Ask, never_asked)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::NoneOn);
    assert!(
        reset_password(&pool, "nobody", SecondFactor::Ask, never_asked)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn told_to_remove_it_a_reset_removes_a_setup_still_waiting_too() {
    let pool = db::open_in_memory().await;
    let (bob, _) = user(&pool, "bob").await;
    two_factor::start_setup(&pool, &bob).await.unwrap().unwrap();
    assert_eq!(
        two_factor::stage(&pool, bob.id).await.unwrap(),
        two_factor::Stage::Waiting
    );
    // Asked or kept, a setup that isn't on is left alone, as before.
    let (_, _, outcome) = reset_password(&pool, "bob", SecondFactor::Ask, never_asked)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::NoneOn);
    assert_eq!(
        two_factor::stage(&pool, bob.id).await.unwrap(),
        two_factor::Stage::Waiting
    );
    let (_, _, outcome) = reset_password(&pool, "bob", SecondFactor::Remove, never_asked)
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::NoneOn, "it was never on");
    assert_eq!(
        two_factor::stage(&pool, bob.id).await.unwrap(),
        two_factor::Stage::Off
    );
}

#[test]
fn only_a_typed_yes_removes_it() {
    for yes in ["y\n", "Y", " yes \n", "Yes"] {
        assert!(says_yes(yes), "{yes:?}");
    }
    for no in ["", "\n", "n", "no", "N", "yep", "YES please"] {
        assert!(!says_yes(no), "{no:?}");
    }
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

#[tokio::test]
async fn the_host_key_command_follows_the_proxy_saved_in_the_web_ui() {
    let pool = db::open_in_memory().await;
    let none = |_: &str| None;
    let values: std::collections::HashMap<String, Option<String>> = [
        ("MELETE_PROXY_HOST", "192.168.1.10"),
        ("MELETE_PROXY_PORT", "1080"),
        ("MELETE_PROXY_USERNAME", "alice"),
        ("MELETE_PROXY_PASSWORD", "a-password"),
        ("MELETE_PROXY_SEEDBOX", "on"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), Some(value.to_owned())))
    .collect();
    crate::settings::save(&pool, "proxy", &values, &none)
        .await
        .unwrap()
        .unwrap();

    let setup = proxy_setup_from(&pool, &none).await.unwrap();
    assert!(setup.uses(crate::proxy::Service::Seedbox));
    let proxy = setup.at.unwrap();
    assert_eq!(proxy.address(), "socks5h://192.168.1.10:1080");
    let (user, password) = proxy.login.unwrap();
    assert_eq!((user.as_str(), password.expose()), ("alice", "a-password"));

    // The environment wins where it's set, as for the server.
    let off = |name: &str| (name == "MELETE_PROXY_SEEDBOX").then(|| "off".to_owned());
    let setup = proxy_setup_from(&pool, &off).await.unwrap();
    assert!(!setup.uses(crate::proxy::Service::Seedbox));
}

#[tokio::test]
async fn the_host_key_command_reads_the_database_without_changing_it() {
    let dir = tempfile::tempdir().unwrap();
    let only = |name: &str| match name {
        "MELETE_PROXY_HOST" => Some("192.168.1.10".to_owned()),
        "MELETE_PROXY_PORT" => Some("1080".to_owned()),
        "MELETE_PROXY_SEEDBOX" => Some("on".to_owned()),
        _ => None,
    };
    // No database yet: the environment's setup, and nothing is made.
    let setup = proxy_setup_in(dir.path(), &only).await.unwrap();
    assert!(setup.uses(crate::proxy::Service::Seedbox));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

    let pool = db::open(dir.path()).await.unwrap();
    let none = |_: &str| None;
    let values: std::collections::HashMap<String, Option<String>> = [
        ("MELETE_PROXY_HOST", "192.168.1.10"),
        ("MELETE_PROXY_PORT", "1080"),
        ("MELETE_PROXY_SEEDBOX", "on"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), Some(value.to_owned())))
    .collect();
    crate::settings::save(&pool, "proxy", &values, &none)
        .await
        .unwrap()
        .unwrap();
    pool.close().await;
    let before = std::fs::read(dir.path().join("melete.db")).unwrap();

    let setup = proxy_setup_in(dir.path(), &none).await.unwrap();
    assert!(setup.uses(crate::proxy::Service::Seedbox));
    assert_eq!(std::fs::read(dir.path().join("melete.db")).unwrap(), before);
    assert!(!dir.path().join(crate::secrets::KEY_FILE).exists());
}
