//! Users for tests, each with a password made at run time, so no test writes a password down:
//! code scanners rightly flag every hard-coded one. Tests about passwords themselves (signing
//! in, changing one, wrong ones) still say theirs.

use sqlx::SqlitePool;

use super::{User, create_user, generate_password};

/// A password made at run time, as one is generated for a new user: nothing in it is written
/// down, not even a prefix.
pub fn password() -> String {
    generate_password().expect("randomness is available")
}

/// Creates the user `name`, an admin when `admin`, with a password no test needs to know.
pub async fn user(pool: &SqlitePool, name: &str, admin: bool) -> User {
    user_with_password(pool, name, admin).await.0
}

/// As [`user`], with its password, for a test that signs in with it.
pub async fn user_with_password(pool: &SqlitePool, name: &str, admin: bool) -> (User, String) {
    let password = password();
    let user = create_user(pool, name, password.clone(), admin)
        .await
        .expect("the test user is created")
        .expect("the test user's name is free");
    (user, password)
}
