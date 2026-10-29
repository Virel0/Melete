//! A soft limit on how long hand-written source files get. A Rust file's code, counted up
//! to its test module (`#[cfg(test)]` then `mod … {`, or `mod …;` for tests in a file of their
//! own), and a web UI file, counted whole, stay at
//! or under [`LIMIT`] lines; vendored files, and tests moved into `tests.rs` files, are left out. Files longer than that when the limit
//! came in are listed with their length then, and may only get shorter: a file that shrinks has
//! its number lowered here, and one under the limit comes off the list. A file grows by being
//! split, not by raising its number.

use std::path::{Path, PathBuf};

const LIMIT: usize = 500;

/// Longer than [`LIMIT`] when the limit came in, by path, with their lines then.
const LONGER: &[(&str, usize)] = &[];

/// Lines of code: a Rust file's up to its test module, a web file's whole.
fn code_lines(path: &Path, text: &str) -> usize {
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    if path.extension().is_some_and(|e| e != "rs") {
        return lines.len();
    }
    for (i, line) in lines.iter().enumerate() {
        if line.trim() != "#[cfg(test)]" {
            continue;
        }
        let next = lines[i + 1..]
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with("#["));
        let is_module = next.is_some_and(|l| {
            // Any visibility: `pub`, `pub(crate)`, `pub(super)` and the like.
            let l = match l.strip_prefix("pub(") {
                Some(rest) => rest.split_once(") ").map_or(l, |(_, after)| after),
                None => l.strip_prefix("pub ").unwrap_or(l),
            };
            // `mod tests {`, or `mod tests;` with the tests in a file of their own.
            l.starts_with("mod ") && (l.ends_with('{') || l.ends_with(';'))
        });
        if is_module {
            return i;
        }
    }
    lines.len()
}

/// Hand-written source files: Rust anywhere under `src/`, and the web UI's scripts and styles.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "vendor") {
                sources(&path, out);
            }
        } else if path.file_name().is_some_and(|n| n == "tests.rs") {
            // A module's tests in a file of their own: tests, not code.
        } else if path.extension().is_some_and(|e| e == "rs")
            || (path.starts_with("src/web")
                && path.extension().is_some_and(|e| e == "js" || e == "css"))
        {
            out.push(path);
        }
    }
}

#[test]
fn source_files_stay_short() {
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
    let mut files = Vec::new();
    sources(Path::new("src"), &mut files);
    files.sort();
    let mut wrong = Vec::new();
    for path in &files {
        let name = path.to_string_lossy().replace('\\', "/");
        let lines = code_lines(path, &std::fs::read_to_string(path).unwrap());
        match LONGER.iter().find(|(listed, _)| *listed == name) {
            None if lines > LIMIT => wrong.push(format!(
                "{name} has {lines} lines of code, over the {LIMIT}-line limit: split it"
            )),
            Some((_, was)) if lines > *was => wrong.push(format!(
                "{name} grew from {was} to {lines} lines: split it rather than raise its number"
            )),
            Some(_) if lines <= LIMIT => wrong.push(format!(
                "{name} is down to {lines} lines, within the limit: take it off LONGER"
            )),
            Some((_, was)) if lines < *was => wrong.push(format!(
                "{name} shrank from {was} to {lines} lines: lower its number in LONGER"
            )),
            _ => {}
        }
    }
    for (listed, _) in LONGER {
        if !files.iter().any(|p| p.to_string_lossy() == *listed) {
            wrong.push(format!("{listed} is listed but gone: take it off LONGER"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn counts_code_up_to_the_test_module() {
    let rust = "fn a() {}\n\n#[cfg(test)]\nfn helper() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n";
    assert_eq!(
        code_lines(Path::new("x.rs"), rust),
        5,
        "a test-only item isn't the module"
    );
    let named = "fn a() {}\n#[cfg(test)]\n#[allow(dead_code)]\npub(crate) mod testing {\n}\n";
    assert_eq!(code_lines(Path::new("x.rs"), named), 1);
    let parent = "fn a() {}\nfn b() {}\n#[cfg(test)]\npub(super) mod tests {\n}\n";
    assert_eq!(code_lines(Path::new("x.rs"), parent), 2);
    let own_file = "fn a() {}\n#[cfg(test)]\nmod tests;\n";
    assert_eq!(code_lines(Path::new("x.rs"), own_file), 1);
    assert_eq!(code_lines(Path::new("x.rs"), "fn a() {}\nfn b() {}\n"), 2);
    assert_eq!(
        code_lines(Path::new("x.js"), "a\nb\n#[cfg(test)]\nmod t {\n"),
        4
    );
}
