//! Major studios: a built-in list of the large film and TV studios and the major anime studios,
//! matched against a title's companies from TMDB by name (any case, the listed name anywhere in
//! the company's: "Warner Bros." matches "Warner Bros. Pictures"). An admin can add names
//! (`MELETE_MAJOR_STUDIOS`, comma-separated).

/// The built-in major studios, labels included, as their names appear in TMDB's companies.
const MAJOR: &[&str] = &[
    // Disney and its labels.
    "Walt Disney",
    "Disney",
    "Pixar",
    "Marvel Studios",
    "Lucasfilm",
    "20th Century",
    "Searchlight Pictures",
    // Warner Bros. Discovery.
    "Warner Bros.",
    "New Line Cinema",
    "HBO",
    "DC Entertainment",
    "DC Films",
    "DC Studios",
    // Comcast.
    "Universal Pictures",
    "Universal Television",
    "Illumination",
    "DreamWorks",
    "Focus Features",
    "NBC",
    // Paramount.
    "Paramount",
    "Nickelodeon",
    "CBS",
    // Sony.
    "Sony Pictures",
    "Columbia Pictures",
    "TriStar",
    "Screen Gems",
    // Others of that size.
    "Lionsgate",
    "Metro-Goldwyn-Mayer",
    "Amazon Studios",
    "Amazon MGM Studios",
    "Netflix",
    "Apple Studios",
    "Apple TV+",
    // Major anime studios and producers.
    "Toho",
    "Toei Animation",
    "Studio Ghibli",
    "Sunrise",
    "Bandai Namco Filmworks",
    "Kyoto Animation",
    "MAPPA",
    "Madhouse",
    "Production I.G",
    "Bones",
    "Wit Studio",
    "ufotable",
    "A-1 Pictures",
    "CloverWorks",
    "Aniplex",
    "Shaft",
    "Trigger",
];

/// Whether a company's name is a major studio's: the built-in list or `extra`.
pub fn is_major(company: &str, extra: &[String]) -> bool {
    let company = company.to_lowercase();
    MAJOR
        .iter()
        .copied()
        .chain(extra.iter().map(String::as_str))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .any(|name| company.contains(&name.to_lowercase()))
}

/// Whether any of a title's companies (a JSON array of names, as stored) is a major studio's.
pub(super) fn of(companies: Option<&str>, extra: &[String]) -> bool {
    let names: Vec<String> = companies
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or_default();
    names.iter().any(|name| is_major(name, extra))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_studios_are_known_by_any_of_their_names() {
        assert!(is_major("Warner Bros. Pictures", &[]));
        assert!(is_major("Walt Disney Animation Studios", &[]));
        assert!(is_major("MAPPA", &[]));
        assert!(is_major("mappa", &[]), "any case");
        assert!(!is_major("A24", &[]));
        assert!(!is_major("Small Films", &[]));
        assert!(is_major("A24", &[" A24 ".into()]), "an admin's additions");
        assert!(
            !is_major("Anything", &[String::new()]),
            "an empty name matches nothing"
        );
        assert!(of(Some(r#"["Small Films","HBO"]"#), &[]));
        assert!(!of(Some(r#"["Small Films"]"#), &[]));
        assert!(!of(None, &[]));
    }
}
