//! Each country's content ratings on one age scale: the youngest age a rating lets watch. A
//! rating not listed here, and not a plain age ("12", "16+", "FSK 16"), can't be read.

use crate::settings::Choice;

/// The countries whose ratings Melete reads, by ISO 3166 code: the United States first, as the
/// default and the rating that counts when the chosen country has none for a title.
pub const COUNTRIES: &[Choice] = &[
    Choice::new("US", "United States"),
    Choice::new("AU", "Australia"),
    Choice::new("BR", "Brazil"),
    Choice::new("CA", "Canada"),
    Choice::new("DK", "Denmark"),
    Choice::new("FI", "Finland"),
    Choice::new("FR", "France"),
    Choice::new("DE", "Germany"),
    Choice::new("IN", "India"),
    Choice::new("IE", "Ireland"),
    Choice::new("IT", "Italy"),
    Choice::new("JP", "Japan"),
    Choice::new("NL", "Netherlands"),
    Choice::new("NZ", "New Zealand"),
    Choice::new("NO", "Norway"),
    Choice::new("RU", "Russia"),
    Choice::new("KR", "South Korea"),
    Choice::new("ES", "Spain"),
    Choice::new("SE", "Sweden"),
    Choice::new("GB", "United Kingdom"),
];

/// The age a country's certification comes to, from 0 (everyone) to 18; None when it can't
/// be read.
pub fn age(country: &str, certification: &str) -> Option<u8> {
    let cert = certification.trim().to_uppercase();
    if let Some(age) = named(&country.to_uppercase(), &cert) {
        return Some(age);
    }
    plain_age(&cert)
}

/// "12", "16+", "FSK 16", "K-12", "VM14", "+13", "U/A 13+": an age written out, after nothing
/// or a prefix some country really writes before one.
fn plain_age(cert: &str) -> Option<u8> {
    let start = cert.find(|c: char| c.is_ascii_digit())?;
    let (prefix, rest) = cert.split_at(start);
    let digits = rest.strip_suffix('+').unwrap_or(rest);
    let prefix = prefix.trim_end_matches([' ', '-']);
    if !["", "+", "FSK", "K", "VM", "U/A"].contains(&prefix)
        || digits.len() > 2
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    digits.parse::<u8>().ok().map(|a| a.min(18))
}

/// Ratings that name rather than number an age, by country.
fn named(country: &str, cert: &str) -> Option<u8> {
    let table: &[(&str, u8)] = match country {
        "US" => &[
            ("G", 0),
            ("PG", 10),
            ("PG-13", 13),
            ("R", 17),
            ("NC-17", 18),
            ("TV-Y", 0),
            ("TV-Y7", 7),
            ("TV-Y7-FV", 7),
            ("TV-G", 0),
            ("TV-PG", 10),
            ("TV-14", 14),
            ("TV-MA", 17),
        ],
        "GB" => &[("U", 0), ("UC", 0), ("PG", 8), ("12A", 12), ("R18", 18)],
        "IE" => &[("G", 0), ("PG", 8), ("12A", 12), ("15A", 15), ("16", 16)],
        "CA" => &[
            ("G", 0),
            ("PG", 8),
            ("14A", 14),
            ("18A", 18),
            ("R", 18),
            ("A", 18),
            ("C", 0),
            ("C8", 8),
            ("14+", 14),
            ("18+", 18),
        ],
        "AU" => &[
            ("G", 0),
            ("P", 0),
            ("C", 0),
            ("PG", 8),
            ("M", 15),
            ("MA15+", 15),
            ("MA 15+", 15),
            ("AV15+", 15),
            ("R18+", 18),
            ("R 18+", 18),
            ("X18+", 18),
        ],
        "NZ" => &[
            ("G", 0),
            ("PG", 8),
            ("M", 16),
            ("R13", 13),
            ("R15", 15),
            ("R16", 16),
            ("R18", 18),
            ("RP13", 13),
            ("RP16", 16),
            ("RP18", 18),
            ("R", 18),
        ],
        "FR" => &[("U", 0), ("TP", 0)],
        "NL" => &[("AL", 0)],
        "ES" => &[("A", 0), ("APTA", 0), ("TP", 0), ("AI", 0), ("X", 18)],
        "IT" => &[("T", 0)],
        "JP" => &[("G", 0), ("PG12", 12), ("R15+", 15), ("R18+", 18)],
        "KR" => &[("ALL", 0), ("RESTRICTED SCREENING", 18)],
        "BR" => &[("L", 0), ("AL", 0)],
        "SE" => &[("BTL", 0)],
        "DK" | "NO" => &[("A", 0)],
        "FI" => &[("S", 0)],
        "IN" => &[("U", 0), ("UA", 12), ("U/A", 12), ("A", 18), ("S", 18)],
        _ => &[],
    };
    table
        .iter()
        .find(|(name, _)| *name == cert)
        .map(|(_, age)| *age)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_ratings_come_to_their_age() {
        assert_eq!(age("US", "PG-13"), Some(13));
        assert_eq!(age("us", "r"), Some(17));
        assert_eq!(age("US", "TV-MA"), Some(17));
        assert_eq!(age("GB", "12A"), Some(12));
        assert_eq!(age("GB", "15"), Some(15));
        assert_eq!(age("AU", "MA15+"), Some(15));
        assert_eq!(age("FR", "U"), Some(0));
        assert_eq!(age("JP", "R18+"), Some(18));
    }

    #[test]
    fn written_out_ages_are_read_anywhere() {
        assert_eq!(age("DE", "12"), Some(12));
        assert_eq!(age("DE", "FSK 16"), Some(16));
        assert_eq!(age("FI", "K-7"), Some(7));
        assert_eq!(age("IT", "VM14"), Some(14));
        assert_eq!(age("BR", "16"), Some(16));
        assert_eq!(age("RU", "6+"), Some(6));
        assert_eq!(age("KR", "15"), Some(15));
        assert_eq!(age("IN", "U/A 13+"), Some(13));
    }

    #[test]
    fn what_cant_be_read_isnt_guessed() {
        for (country, cert) in [
            ("US", "NR"),
            ("US", "Unrated"),
            ("XX", "PG-13"),
            ("DE", ""),
            ("DE", "123"),
            ("GB", "E"),
            ("CA", "Exempt"),
            ("US", "Approved 12"),
        ] {
            assert_eq!(age(country, cert), None, "{country} {cert}");
        }
    }
}
