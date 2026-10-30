//! The account, import, backup and restore commands, run inside the container: `docker exec melete melete …`
//! (a restore with Melete stopped, through `docker compose run`).

use anyhow::bail;

use crate::{aoide, auth, backup, two_factor};

pub(crate) const USAGE: &str = "usage:
  melete                                  run the server
  melete user add <name> [--admin]        create a user with a generated password
  melete user reset-password <name> [--remove-second-factor | --keep-second-factor]
                                          set a new generated password and sign the user out everywhere;
                                          asks whether to remove their second factor, when they have one
  melete backup                           copy the database into the data folder's backups
  melete restore [<copy>]                 list the copies, or put one in place of the database, with
                                          Melete stopped (docker compose run --rm melete melete restore <copy>)
  melete seedbox host-key <host> [port]   show the key a seedbox presents, to pin it (sends nothing)
  melete import aoide <aoide-sync.db> [--user <jellyfin-id>=<name>]... [--skip-user <jellyfin-id>]... [--apply]
                                          import the Jellyfin aoide-sidecar's sync data (a dry run without --apply)";

/// Account commands, run inside the container: `docker exec melete melete user add <name> --admin`.
pub(crate) async fn run_command(pool: &sqlx::SqlitePool, args: &[String]) -> anyhow::Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["user", "add", name, flags @ ..] if matches!(flags, [] | ["--admin"]) => {
            if let Err(rule) = auth::validate_username(name) {
                bail!("{rule}");
            }
            let password = auth::generate_password()?;
            let is_admin = !flags.is_empty();
            if auth::create_user(pool, name, password.clone(), is_admin)
                .await?
                .is_none()
            {
                bail!("the username {name:?} is taken");
            }
            let role = if is_admin { "admin" } else { "user" };
            println!("Created {role} {name}. Password: {password}");
            println!("Sign in and change it with PUT /api/v1/me/password.");
        }
        ["user", "reset-password", name, flags @ ..] => {
            let Some(second_factor) = SecondFactor::from_flags(flags) else {
                bail!("{USAGE}");
            };
            let (user, password, second_factor) =
                reset_password(pool, name, second_factor, ask_on_terminal).await?;
            println!("New password for {}: {password}", user.username);
            println!("All of their sessions and API keys were signed out.");
            println!("A sign-in lockout already running ends within 15 minutes.");
            match second_factor {
                Outcome::NoneOn => {}
                Outcome::Removed => println!(
                    "Their two-factor sign-in was removed: the password alone signs in now."
                ),
                Outcome::Kept => {
                    println!(
                        "Their two-factor sign-in stays on: signing in still asks for a code."
                    );
                    println!("To remove it, run this again with --remove-second-factor.");
                }
            }
        }
        ["import", "aoide", rest @ ..] => aoide::import::run(pool, rest).await?,
        ["backup"] => {
            let data_dir = crate::config::Config::data_dir_from_env();
            let copy = backup::by_hand(pool, &data_dir).await?;
            let path = backup::folder(&data_dir)?.join(&copy.file);
            println!(
                "Copied the database to {} ({} bytes).",
                path.display(),
                copy.bytes
            );
            println!("It's kept until you remove it.");
        }
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

/// What `user reset-password` does about a second factor the user has on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecondFactor {
    /// Remove it too: the way back for a lost device.
    Remove,
    /// Leave it on.
    Keep,
    /// Ask, when there's someone at a terminal to answer.
    Ask,
}

/// What `user reset-password` did about a second factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The user had none on.
    NoneOn,
    /// It was removed.
    Removed,
    /// It stays on.
    Kept,
}

impl SecondFactor {
    /// What the command line's flags say; None for flags it doesn't take.
    fn from_flags(flags: &[&str]) -> Option<Self> {
        match flags {
            [] => Some(Self::Ask),
            ["--remove-second-factor"] => Some(Self::Remove),
            ["--keep-second-factor"] => Some(Self::Keep),
            _ => None,
        }
    }
}

/// Sets a new password for `name` and signs them out everywhere; with a second factor on,
/// removes it too when told to or when `ask` says yes. Asked before anything changes, so one
/// run does both. Answers the user, their new password and what happened to the second factor.
pub(crate) async fn reset_password(
    pool: &sqlx::SqlitePool,
    name: &str,
    second_factor: SecondFactor,
    ask: impl FnOnce() -> anyhow::Result<bool>,
) -> anyhow::Result<(auth::User, String, Outcome)> {
    let Some(user) = auth::find_user(pool, name).await? else {
        bail!("no user named {name:?}");
    };
    let has_one = two_factor::is_on(pool, user.id).await?;
    let remove = has_one
        && match second_factor {
            SecondFactor::Remove => true,
            SecondFactor::Keep => false,
            SecondFactor::Ask => ask()?,
        };
    let password = auth::generate_password()?;
    auth::set_password(pool, user.id, password.clone(), None).await?;
    let outcome = if remove {
        two_factor::remove(pool, user.id, two_factor::MadeFrom::Server).await?;
        Outcome::Removed
    } else if has_one {
        Outcome::Kept
    } else {
        Outcome::NoneOn
    };
    Ok((user, password, outcome))
}

/// Asks on the terminal whether to remove the user's second factor; no when nobody is there to
/// answer (`docker exec` without `-it`).
fn ask_on_terminal() -> anyhow::Result<bool> {
    use std::io::{BufRead, IsTerminal, Write};
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return Ok(false);
    }
    print!("They have two-factor sign-in on. Remove it too, for a lost device? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    stdin.lock().read_line(&mut answer)?;
    Ok(says_yes(&answer))
}

/// Whether a typed answer is yes; anything else, an empty line included, is no.
fn says_yes(answer: &str) -> bool {
    matches!(answer.trim(), "y" | "Y" | "yes" | "Yes")
}

/// Runs `melete restore [<copy>]` when that's the command, saying whether it was. It never
/// opens the database the usual way, which would migrate it first.
pub(crate) async fn restore_command(args: &[String]) -> anyhow::Result<bool> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let data_dir = crate::config::Config::data_dir_from_env();
    let folder = data_dir.join(backup::FOLDER);
    match args.as_slice() {
        ["restore"] => {
            let copies = backup::restore::copies(&data_dir)?;
            if copies.is_empty() {
                println!("There are no copies in {}.", folder.display());
                return Ok(true);
            }
            println!("Copies in {}, newest first:", folder.display());
            for (name, bytes, _) in &copies {
                println!("  {name}  ({bytes} bytes)");
            }
            println!("Restore one, with Melete stopped: melete restore <copy>");
        }
        ["restore", chosen] => {
            let restored = backup::restore::restore(&data_dir, chosen, &sqlx::migrate!()).await?;
            say_restored(&data_dir, &restored);
        }
        ["restore", ..] => bail!("{USAGE}"),
        _ => return Ok(false),
    }
    Ok(true)
}

/// Says what a restore did, step by step.
fn say_restored(data_dir: &std::path::Path, restored: &backup::restore::Restored) {
    use backup::restore::Kept;
    let folder = data_dir.join(backup::FOLDER);
    println!(
        "Checked {}: it passes SQLite's integrity check and holds migrations up to {:04}.",
        restored.from.display(),
        restored.version
    );
    match &restored.kept {
        Kept::Nothing => println!("There was no database to keep."),
        Kept::Copied(copy) => println!(
            "Kept the database that was there as {} ({} bytes), until you remove it.",
            folder.join(&copy.file).display(),
            copy.bytes
        ),
        Kept::Moved { files, why } => {
            println!("SQLite couldn't copy the database that was there ({why}),");
            println!(
                "so its files were moved as they were into {}:",
                folder.display()
            );
            for file in files {
                println!("  {file}");
            }
        }
    }
    for file in &restored.removed {
        println!("Removed {}.", data_dir.join(file).display());
    }
    println!(
        "Put the copy in place of {}.",
        data_dir.join("melete.db").display()
    );
    if restored.behind {
        println!(
            "The copy is from an older version: when Melete starts it copies it again, then applies the newer migrations."
        );
    }
    println!("Start Melete again to use it.");
}

#[cfg(test)]
mod tests;
