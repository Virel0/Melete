//! Which compose service Melete's image runs as (`MELETE_SERVICE`, set by each compose file).
//! An image digest put on another service's variable (the trailer downloader's on
//! `MELETE_IMAGE`, say) would otherwise start the wrong program, which then fails on something
//! that doesn't point at the cause. Each of Melete's images checks it at start; this is the
//! server's own check. Run outside compose, with it unset, nothing is checked.

/// Melete's images: each service's name, what it is in words, and the variable naming its
/// image in `.env`.
const IMAGES: [(&str, &str, &str); 3] = [
    ("melete", "Melete", "MELETE_IMAGE"),
    ("stems", "the stem separator", "MELETE_STEMS_IMAGE"),
    (
        "trailers",
        "the trailer downloader",
        "MELETE_TRAILERS_IMAGE",
    ),
];

/// Why Melete's image shouldn't run as `service`, when it shouldn't: as another of Melete's
/// services, or one it doesn't know.
pub fn wrong(service: Option<&str>) -> Option<String> {
    let service = service.map(str::trim).filter(|s| !s.is_empty())?;
    if service == "melete" {
        return None;
    }
    Some(match IMAGES.iter().find(|(name, _, _)| *name == service) {
        Some((_, what, variable)) => format!(
            "this is Melete's image, but it's running as {what}: check {variable} in .env, which names {what}'s image"
        ),
        None => format!(
            "this is Melete's image, but MELETE_SERVICE says it's running as {service:?}, which isn't one of Melete's services (melete, stems, trailers): check MELETE_IMAGE in .env and the compose files"
        ),
    })
}

/// Stops before anything starts when Melete's image runs as another service.
///
/// # Errors
/// Saying which service it runs as and which variable to check.
pub fn check() -> anyhow::Result<()> {
    match wrong(std::env::var("MELETE_SERVICE").ok().as_deref()) {
        Some(why) => anyhow::bail!(why),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn melete_or_nothing_set_starts() {
        for service in [None, Some("melete"), Some(""), Some(" melete ")] {
            assert_eq!(wrong(service), None, "{service:?}");
        }
    }

    #[test]
    fn another_service_stops_naming_its_variable() {
        let why = wrong(Some("trailers")).unwrap();
        assert!(
            why.contains("running as the trailer downloader")
                && why.contains("check MELETE_TRAILERS_IMAGE"),
            "{why}"
        );
        let why = wrong(Some("stems")).unwrap();
        assert!(why.contains("check MELETE_STEMS_IMAGE"), "{why}");
        let why = wrong(Some("slskd")).unwrap();
        assert!(
            why.contains("\"slskd\"") && why.contains("MELETE_IMAGE"),
            "{why}"
        );
    }
}
