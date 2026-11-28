//! The release page's notes (`.github/release-notes.sh`, run by the release workflow) carry
//! every image's full digest as a line ready to paste into `.env`, each under the variable its
//! compose file reads.

use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(name: &str) -> String {
    std::fs::read_to_string(root().join(name)).unwrap()
}

/// What the script prints, or why it refused.
fn notes(args: &[&str]) -> Result<String, String> {
    let out = Command::new("bash")
        .arg(root().join(".github/release-notes.sh"))
        .args(args)
        .output()
        .expect("bash runs");
    if out.status.success() {
        Ok(String::from_utf8(out.stdout).unwrap())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

const IMAGE: &str = "ghcr.io/owner/melete";
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn every_image_is_a_whole_env_line_under_its_compose_variable() {
    for (part, variable, file, tag) in [
        ("melete", "MELETE_IMAGE", "compose.yaml", "1.2.3"),
        (
            "stems",
            "MELETE_STEMS_IMAGE",
            "compose.stems.yaml",
            "1.2.3-stems",
        ),
        (
            "trailers",
            "MELETE_TRAILERS_IMAGE",
            "compose.trailers.yaml",
            "1.2.3-trailers",
        ),
    ] {
        let said = notes(&[part, IMAGE, "1.2.3", DIGEST, "owner"]).unwrap();
        let line = format!("{variable}={IMAGE}:{tag}@{DIGEST}");
        assert!(said.lines().any(|l| l == line), "{part}: {said}");
        // The variable is the one its compose file reads the image from.
        assert!(
            read(file).contains(&format!("image: ${{{variable}:-")),
            "{file} reads {variable}"
        );
    }
}

#[test]
fn the_notes_still_say_how_to_check_the_image() {
    let said = notes(&["melete", IMAGE, "1.2.3", DIGEST, "owner"]).unwrap();
    assert!(
        said.contains(&format!(
            "gh attestation verify oci://{IMAGE}:1.2.3@{DIGEST} --owner owner"
        )),
        "{said}"
    );
    assert!(said.contains("SHA256SUMS"), "{said}");
}

#[test]
fn a_short_or_missing_digest_is_refused() {
    for digest in ["sha256:0123456789ab", "0123456789abcdef", ""] {
        let refused = notes(&["stems", IMAGE, "1.2.3", digest]).unwrap_err();
        assert!(!refused.is_empty(), "{digest:?}");
    }
    assert!(notes(&["slskd", IMAGE, "1.2.3", DIGEST]).is_err());
}

#[test]
fn the_release_workflow_writes_its_notes_with_the_script() {
    let workflow = read(".github/workflows/release.yml");
    for call in [
        r#"bash .github/release-notes.sh melete "$IMAGE" "$VERSION" "$DIGEST" "$GITHUB_REPOSITORY_OWNER" > notes.md"#,
        r#"bash .github/release-notes.sh "$PART" "$IMAGE" "$VERSION" "$DIGEST" >> notes.md"#,
    ] {
        assert!(workflow.contains(call), "{call}");
    }
}
