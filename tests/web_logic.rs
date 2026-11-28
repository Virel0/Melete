//! The web UI's pure logic, tested with Node's own test runner: the files in `tests/web/`. They
//! need `node` (20 or later) on the `PATH`, as other tests need `ffmpeg`.

use std::path::Path;
use std::process::Command;

#[test]
fn the_web_uis_logic_passes_its_node_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files: Vec<_> = std::fs::read_dir(root.join("tests/web"))
        .expect("tests/web is there")
        .map(|entry| entry.expect("tests/web is readable").path())
        .filter(|path| path.to_string_lossy().ends_with(".test.mjs"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no tests in tests/web");
    let out = Command::new("node")
        .arg("--test")
        .args(&files)
        .current_dir(root)
        .output()
        .expect("node is on the PATH");
    assert!(
        out.status.success(),
        "node --test failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
