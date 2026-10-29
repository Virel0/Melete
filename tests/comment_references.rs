//! Comments say what the code does and why, in their own words. They don't send the reader to
//! the issue tracker or the design document for it: an issue number (`#123`), a step of the plan
//! ("step 10a") or a row of the decisions table ("decision 37") is history, and git keeps that.
//! Checked in the hand-written sources (Rust, the web UI's scripts, styles and page) and in the
//! API contract's descriptions. Merged migrations are never edited, so they're left out.

use std::path::{Path, PathBuf};

/// The first citation of an issue, a step of the plan or a decision in `text`, if any. Issue
/// numbers never start with 0, so a colour such as `#000` isn't one.
fn cites(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '#');
        if let Some(number) = bare.strip_prefix('#')
            && !number.is_empty()
            && number.len() <= 4
            && !number.starts_with('0')
            && number.chars().all(|c| c.is_ascii_digit())
        {
            return Some(bare.to_owned());
        }
        let lower = bare.to_lowercase();
        if matches!(lower.as_str(), "step" | "steps" | "decision" | "decisions")
            && let Some(next) = words.get(i + 1)
        {
            let next = next.trim_matches(|c: char| !c.is_alphanumeric());
            let digits = next.trim_end_matches(|c: char| c.is_ascii_lowercase());
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return Some(format!("{bare} {next}"));
            }
        }
    }
    None
}

/// The comment text on each line of a source file, with its line number: `//` comments (after
/// whitespace or at the start, so a URL's `//` inside a string isn't one), `/* … */` and
/// `<!-- … -->` blocks, whatever they share a line with.
fn comments(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut block: Option<&str> = None;
    for (n, line) in text.lines().enumerate() {
        let mut rest = line;
        loop {
            if let Some(end) = block {
                match rest.find(end) {
                    Some(at) => {
                        found.push((n + 1, rest[..at].to_owned()));
                        rest = &rest[at + end.len()..];
                        block = None;
                    }
                    None => {
                        found.push((n + 1, rest.to_owned()));
                        break;
                    }
                }
            }
            let line_comment = rest
                .match_indices("//")
                .find(|(at, _)| *at == 0 || rest[..*at].ends_with(char::is_whitespace))
                .map(|(at, _)| at);
            let opener = [("/*", "*/"), ("<!--", "-->")]
                .iter()
                .filter_map(|(open, close)| rest.find(open).map(|at| (at, *open, *close)))
                .min_by_key(|(at, ..)| *at);
            match (line_comment, opener) {
                (Some(c), Some((o, ..))) if c < o => {
                    found.push((n + 1, rest[c..].to_owned()));
                    break;
                }
                (_, Some((o, open, close))) => {
                    rest = &rest[o + open.len()..];
                    block = Some(close);
                }
                (Some(c), None) => {
                    found.push((n + 1, rest[c..].to_owned()));
                    break;
                }
                (None, None) => break,
            }
        }
    }
    found
}

/// The hand-written sources: Rust under `src/` and `tests/`, and the web UI's files.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "vendor") {
                sources(&path, out);
            }
        } else if path
            .extension()
            .is_some_and(|e| ["rs", "js", "css", "html"].iter().any(|x| e == *x))
        {
            out.push(path);
        }
    }
}

#[test]
fn comments_cite_no_issue_step_or_decision() {
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
    let mut files = Vec::new();
    sources(Path::new("src"), &mut files);
    sources(Path::new("tests"), &mut files);
    files.sort();
    let mut wrong = Vec::new();
    // This file quotes citations on purpose, to test the check.
    files.retain(|p| p != Path::new("tests/comment_references.rs"));
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        for (line, comment) in comments(&text) {
            if let Some(citation) = cites(&comment) {
                wrong.push(format!("{}:{line}: {citation}", path.display()));
            }
        }
    }
    let contract: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string("src/api/openapi.json").unwrap()).unwrap();
    let mut texts = Vec::new();
    fn walk(value: &serde_json::Value, texts: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    match value {
                        serde_json::Value::String(text)
                            if key == "description" || key == "summary" =>
                        {
                            texts.push(text.clone())
                        }
                        _ => walk(value, texts),
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, texts)),
            _ => {}
        }
    }
    walk(&contract, &mut texts);
    for text in texts {
        if let Some(citation) = cites(&text) {
            wrong.push(format!("src/api/openapi.json: {citation} in {text:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "\ncomments that cite an issue, step or decision instead of saying why:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn finds_citations_only_in_comments() {
    assert_eq!(cites("as #98 asked").as_deref(), Some("#98"));
    assert_eq!(cites("(step 10a)").as_deref(), Some("step 10a"));
    assert_eq!(cites("see decision 37.").as_deref(), Some("decision 37"));
    assert_eq!(cites("in steps of five; colour #fff"), None);
    assert_eq!(
        cites("pure black (#000)"),
        None,
        "issue numbers never start with 0"
    );
    let rust = "let url = \"https://x/#1\"; // fetched once\n/* kept (#12) */ let a = 1;\n";
    let found: Vec<String> = comments(rust).into_iter().map(|(_, c)| c).collect();
    assert_eq!(
        found,
        vec!["// fetched once".to_owned(), " kept (#12) ".to_owned()]
    );
    let css = "a { background: #000; /* plain black */ }\n";
    assert!(comments(css).iter().all(|(_, c)| cites(c).is_none()));
}
