//! Each of Melete's images checks at start which compose service it runs as
//! (`MELETE_SERVICE`, set by each compose file), so an image digest put on another service's
//! variable stops at once with a message naming the variable to check, rather than starting the
//! wrong program.

use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The exit status and what was said on stderr.
fn run(command: &mut Command) -> (bool, String) {
    let out = command.output().expect("runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn each_compose_service_says_which_it_is() {
    for (file, service) in [
        ("compose.yaml", "melete"),
        ("compose.stems.yaml", "stems"),
        ("compose.trailers.yaml", "trailers"),
    ] {
        let text = std::fs::read_to_string(root().join(file)).unwrap();
        let said: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("MELETE_SERVICE:"))
            .collect();
        assert_eq!(said, [format!("MELETE_SERVICE: {service}")], "{file}");
    }
}

#[test]
fn meletes_image_running_as_another_service_stops_saying_which_variable() {
    let data = tempfile::tempdir().unwrap();
    // With a command, so that without the check it would stop at that rather than serve.
    let (ok, said) = run(Command::new(env!("CARGO_BIN_EXE_melete"))
        .args(["no-such-command"])
        .env("MELETE_SERVICE", "trailers")
        .env("MELETE_DATA_DIR", data.path()));
    assert!(!ok);
    assert!(
        said.contains(
            "this is Melete's image, but it's running as the trailer downloader: check MELETE_TRAILERS_IMAGE"
        ),
        "{said}"
    );
    // Stopped before anything started: no database made.
    assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    // As itself, it goes on to its command as usual.
    let (_, said) = run(Command::new(env!("CARGO_BIN_EXE_melete"))
        .args(["no-such-command"])
        .env("MELETE_SERVICE", "melete")
        .env("MELETE_DATA_DIR", data.path()));
    assert!(!said.contains("this is Melete's image"), "{said}");
}

#[test]
fn the_separator_and_downloader_images_running_as_melete_stop_saying_so() {
    for (script, what) in [
        ("stems/separate.py", "the stem separator"),
        ("trailers/fetch.py", "the trailer downloader"),
    ] {
        let (ok, said) = run(Command::new("python3")
            .arg("-I")
            .arg(root().join(script))
            .env("MELETE_SERVICE", "melete"));
        assert!(!ok, "{script}");
        assert!(
            said.contains(&format!(
                "this is {what}'s image, but it's running as Melete: check MELETE_IMAGE"
            )),
            "{script}: {said}"
        );
    }
}
