//! What an install runs: `compose.yaml` pulls the published release of this version unless
//! `.env` names another image, and building from source is an override of its own, so neither
//! choice means editing a tracked file.

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
