//! What an install runs: `compose.yaml` pulls the published release of this version unless
//! `.env` names another image, and building from source is an override of its own, so neither
//! choice means editing a tracked file. The same goes for the user Melete runs as.

use std::path::Path;

fn read(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The service's lines in a compose file that start with `key:`, trimmed.
fn lines_with<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with(&format!("{key}:")))
        .collect()
}

#[test]
fn compose_runs_this_versions_release_unless_env_names_another() {
    let compose = read("compose.yaml");
    let wanted = format!(
        "image: ${{MELETE_IMAGE:-ghcr.io/virel0/melete:{}}}",
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        lines_with(&compose, "image"),
        [wanted.as_str()],
        "compose.yaml should run the release Cargo.toml names"
    );
    assert!(
        lines_with(&compose, "build").is_empty(),
        "building belongs in compose.build.yaml"
    );
}

#[test]
fn building_from_source_is_an_override() {
    let build = read("compose.build.yaml");
    assert_eq!(lines_with(&build, "build"), ["build: ."]);
    assert_eq!(lines_with(&build, "image"), ["image: melete:dev"]);
}

/// What `user:` says in every container that reads or writes Melete's files.
const MELETES_USER: &str = r#"user: "${MELETE_USER:-10001:10001}""#;

#[test]
fn every_container_sharing_meletes_files_runs_as_meletes_user() {
    for file in [
        "compose.yaml",
        "compose.stems.yaml",
        "compose.motion.yaml",
        "compose.soulseek.yaml",
    ] {
        assert_eq!(
            lines_with(&read(file), "user"),
            [MELETES_USER],
            "{file}: one container, as MELETE_USER says"
        );
    }
}

#[test]
fn the_default_user_is_the_images_own() {
    let dockerfile = read("Dockerfile");
    let has = |text: &str| dockerfile.lines().any(|l| l.contains(text));
    assert!(has("groupadd --system --gid 10001 melete"), "its group");
    assert!(has("useradd --system --uid 10001 --gid melete"), "its user");
    assert!(has("USER melete"));
}
