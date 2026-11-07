use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;

fn page() -> &'static str {
    super::super::MODULES
        .iter()
        .find(|(name, _)| *name == "licences.js")
        .expect("licences.js is a module")
        .1
}

/// `==== name version licence` headings in JASSUB's licence file: (name, version, licence).
fn jassub_headings() -> Vec<(String, String, String)> {
    JASSUB
        .lines()
        .filter_map(|line| line.strip_prefix("==== "))
        .filter(|rest| !rest.contains(':'))
        .map(|rest| {
            let mut words = rest.splitn(3, ' ');
            let name = words.next().unwrap().to_string();
            let version = words.next().unwrap().to_string();
            (name, version, words.next().unwrap().to_string())
        })
        .collect()
}

#[tokio::test]
async fn each_licence_text_is_served_as_plain_text_unchanged() {
    for (path, body) in [
        ("/web/licences/hls.js.txt", HLS),
        ("/web/licences/jassub.txt", JASSUB),
    ] {
        let request = Request::get(path).body(Body::empty()).unwrap();
        let response = routes().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let headers = response.headers().clone();
        assert_eq!(
            headers[header::CONTENT_TYPE],
            "text/plain; charset=utf-8",
            "{path}"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::CONTENT_SECURITY_POLICY], POLICY);
        let served = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(served, body.as_bytes(), "{path}");
        // The page fetches exactly these.
        assert!(page().contains(&format!("path: \"{path}\"")), "{path}");
    }
}

/// Every library JASSUB's licence file names is on the page with the version and licence the
/// file gives, and so is the font it ships.
#[test]
fn the_page_names_what_jassubs_licence_file_names() {
    let headings = jassub_headings();
    assert_eq!(headings.len(), 4, "{headings:?}");
    for (name, version, licence) in headings {
        let entry = page()
            .find(&format!("name: \"{name}\",\n    version: \"{version}\",\n"))
            .unwrap_or_else(|| panic!("{name} {version} is on the page"));
        let rest = &page()[entry..];
        let said = rest.split("licence: \"").nth(1).unwrap();
        assert_eq!(said.split('"').next().unwrap(), licence, "{name}");
    }
    // The WebAssembly's licences, as the file states them for the package.
    let stated = JASSUB.lines().nth(2).unwrap();
    assert!(stated.starts_with("LGPL-2.1-or-later AND"), "{stated}");
    assert!(page().contains(&format!("licence: \"{stated}\"")));
    // The font: SIL Open Font License 1.1 is OFL-1.1 in SPDX.
    assert!(JASSUB.contains("==== default.woff2: Liberation Sans, SIL Open Font License 1.1"));
    let font = page().find("name: \"Liberation Sans\"").unwrap();
    assert!(page()[font..].contains("licence: \"OFL-1.1\""));
}

/// hls.js is on the page as the version its file name says, under the licence its LICENSE
/// file gives.
#[test]
fn the_page_names_hls_js_as_its_licence_file_does() {
    assert!(HLS.contains("Licensed under the Apache License, Version 2.0"));
    assert!(
        page().contains("name: \"hls.js\",\n    version: \"1.7.3\",\n    licence: \"Apache-2.0\",")
    );
}

/// A library added to `src/web/vendor` has to be named on the page too, by name and version.
#[test]
fn every_vendored_library_is_on_the_page() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web/vendor");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let file = entry.unwrap().file_name().into_string().unwrap();
        if file.contains("LICENSE") {
            continue;
        }
        let stem = file.strip_suffix(".mjs").unwrap_or(&file);
        let (name, version) = stem.rsplit_once('-').unwrap();
        assert!(
            page().contains(&format!("name: \"{name}")),
            "{file}: no library called {name} on the page"
        );
        assert!(
            page().contains(&format!("version: \"{version}\"")),
            "{file}: version {version} isn't on the page"
        );
        seen += 1;
    }
    assert_eq!(seen, 2);
}

/// Each licence on the page is one a vendored file gives; none is made up.
#[test]
fn every_licence_on_the_page_comes_from_a_vendored_file() {
    let mut known: Vec<String> = jassub_headings().into_iter().map(|h| h.2).collect();
    known.push(JASSUB.lines().nth(2).unwrap().to_string());
    known.push("Apache-2.0".into());
    known.push("OFL-1.1".into());
    let given: Vec<&str> = page()
        .split("licence: \"")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap())
        .collect();
    assert_eq!(given.len(), 6);
    for licence in given {
        assert!(known.iter().any(|k| k == licence), "{licence}");
    }
}
