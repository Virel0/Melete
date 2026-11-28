//! `melete --help` (or `-h`, or `help`) prints the usage plainly and exits 0, before anything
//! is opened: it's asked for, not a mistake.

use std::process::Command;

#[test]
fn help_prints_the_usage_and_succeeds() {
    for asked in ["--help", "-h", "help"] {
        let data = tempfile::tempdir().unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_melete"))
            .arg(asked)
            .env("MELETE_DATA_DIR", data.path())
            .env_remove("MELETE_SERVICE")
            .output()
            .expect("runs");
        let (stdout, stderr) = (
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(out.status.success(), "{asked}: {stderr}");
        assert!(stdout.starts_with("usage:"), "{asked}: {stdout}");
        assert!(
            stdout.contains("melete user add <name> [--admin]"),
            "{stdout}"
        );
        assert!(!stderr.contains("Error"), "{asked}: {stderr}");
        // Nothing opened or made: the data folder is as it was.
        assert_eq!(
            std::fs::read_dir(data.path()).unwrap().count(),
            0,
            "{asked}"
        );
    }
}

#[test]
fn an_unknown_command_still_fails_with_the_usage() {
    let data = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_melete"))
        .arg("--no-such-flag")
        .env("MELETE_DATA_DIR", data.path())
        .env_remove("MELETE_SERVICE")
        .output()
        .expect("runs");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
}
