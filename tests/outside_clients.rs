//! Every HTTP client Melete builds starts in its proxy module (`proxy::builder`), which sends the
//! service through the proxy when an admin chose that and keeps it there when the proxy is down.
//! A client built anywhere else would skip that choice and could go direct, so the hand-written
//! Rust under `src/` never builds one itself, nor sets a client's proxy.

use std::path::{Path, PathBuf};

/// What builds an HTTP client, or picks its proxy, outside the proxy module.
const FORBIDDEN: &[&str] = &[
    "Client::builder(",
    "ClientBuilder::new(",
    "reqwest::Client::new(",
    "reqwest::Client::default(",
    "reqwest::get(",
    "reqwest::blocking",
    "use reqwest::Client",
    ".proxy(",
    ".no_proxy(",
    "Proxy::all(",
    "Proxy::custom(",
];

/// Where they may be: the proxy module itself.
fn allowed(path: &Path) -> bool {
    path == Path::new("src/proxy.rs") || path.starts_with("src/proxy/")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Each line of `text` that builds a client or picks a proxy, with its number.
fn builds_a_client(text: &str) -> Vec<(usize, &'static str)> {
    let mut found = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        if let Some(what) = FORBIDDEN.iter().find(|f| code.contains(*f)) {
            found.push((n + 1, *what));
        }
    }
    found
}

#[test]
fn every_http_client_starts_in_the_proxy_module() {
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
    let mut files = Vec::new();
    rust_files(Path::new("src"), &mut files);
    files.sort();
    let mut wrong = Vec::new();
    for path in files.iter().filter(|p| !allowed(p)) {
        let text = std::fs::read_to_string(path).unwrap();
        for (line, what) in builds_a_client(&text) {
            wrong.push(format!(
                "{}:{line} uses {what}: start the client with crate::proxy::builder(<service>) so the proxy choice applies",
                path.display()
            ));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn finds_a_client_built_on_its_own() {
    let found = |text| {
        builds_a_client(text)
            .into_iter()
            .map(|(n, _)| n)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        found("let http = reqwest::Client::builder()\n    .build()?;"),
        [1]
    );
    assert_eq!(found("use reqwest::Client;\nlet c = Client::new();"), [1]);
    assert_eq!(found("let c = reqwest::Client::new();"), [1]);
    assert_eq!(found("builder\n    .no_proxy()\n"), [2]);
    assert_eq!(
        found("// reqwest::Client::builder() is not used here"),
        Vec::<usize>::new()
    );
    assert_eq!(
        found("let http = crate::proxy::builder(crate::proxy::Service::Tmdb)?\n    .build()?;"),
        Vec::<usize>::new()
    );
    assert!(allowed(Path::new("src/proxy/check.rs")) && !allowed(Path::new("src/tmdb.rs")));
}
