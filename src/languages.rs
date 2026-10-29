//! Language codes as tracks and people give them, made comparable: ISO 639-1 ("ja"), ISO 639-2
//! in either form ("jpn"; "fre" and "fra"), and a region after a hyphen ("pt-BR", "es-419").
//! A region is kept apart, so Brazilian and European Portuguese never stand in for each other.

/// A language as Melete compares it: the ISO 639-1 code where there is one (else the three
/// letters), lower case, and the region, if any, upper case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// "ja", "pt", or three letters for a language without a two-letter code.
    pub base: String,
    /// "BR", "419"; None when the code names no region.
    pub region: Option<String>,
}

impl Language {
    /// Reads a code such as "ja", "jpn", "fre", "pt-BR" or "pt_br". None for anything else, and
    /// for codes that name no language ("und", "mul", "zxx", "mis"), so an untagged track is
    /// never taken for a match.
    pub fn parse(code: &str) -> Option<Language> {
        let code = code.trim().replace('_', "-");
        let (base, region) = match code.split_once('-') {
            Some((base, region)) => (base, Some(region)),
            None => (code.as_str(), None),
        };
        if !(2..=3).contains(&base.len()) || !base.bytes().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        let base = base.to_ascii_lowercase();
        if ["und", "mul", "zxx", "mis", "qaa"].contains(&base.as_str()) {
            return None;
        }
        let region = match region {
            None => None,
            Some(r) => Some(region_code(r)?),
        };
        let base = if base.len() == 3 {
            CODES
                .iter()
                .find(|(_, t, b)| *t == base || *b == base)
                .map_or(base, |(one, _, _)| (*one).to_owned())
        } else {
            base
        };
        Some(Language { base, region })
    }

    /// The code as Melete gives it back: "ja", "pt-BR".
    pub fn code(&self) -> String {
        match &self.region {
            Some(region) => format!("{}-{region}", self.base),
            None => self.base.clone(),
        }
    }

    /// How well a track in `track`'s language answers this wanted language: 2 for the same
    /// language and region, 1 when one of the two names no region, None for another language
    /// or another region of it.
    pub fn answers(&self, track: &Language) -> Option<u8> {
        if self.base != track.base {
            return None;
        }
        match (&self.region, &track.region) {
            (Some(a), Some(b)) if a != b => None,
            (Some(_), Some(_)) | (None, None) => Some(2),
            _ => Some(1),
        }
    }
}

/// A region as two letters ("BR") or three digits ("419"), upper case.
fn region_code(region: &str) -> Option<String> {
    let letters = region.len() == 2 && region.bytes().all(|b| b.is_ascii_alphabetic());
    let digits = region.len() == 3 && region.bytes().all(|b| b.is_ascii_digit());
    (letters || digits).then(|| region.to_ascii_uppercase())
}

/// ISO 639-1 codes with their ISO 639-2 terminology and bibliographic codes (the same where
/// there's one).
const CODES: &[(&str, &str, &str)] = &[
    ("af", "afr", "afr"),
    ("am", "amh", "amh"),
    ("ar", "ara", "ara"),
    ("az", "aze", "aze"),
    ("be", "bel", "bel"),
    ("bg", "bul", "bul"),
    ("bn", "ben", "ben"),
    ("bo", "bod", "tib"),
    ("bs", "bos", "bos"),
    ("ca", "cat", "cat"),
    ("cs", "ces", "cze"),
    ("cy", "cym", "wel"),
    ("da", "dan", "dan"),
    ("de", "deu", "ger"),
    ("el", "ell", "gre"),
    ("en", "eng", "eng"),
    ("eo", "epo", "epo"),
    ("es", "spa", "spa"),
    ("et", "est", "est"),
    ("eu", "eus", "baq"),
    ("fa", "fas", "per"),
    ("fi", "fin", "fin"),
    ("fil", "fil", "fil"),
    ("fr", "fra", "fre"),
    ("ga", "gle", "gle"),
    ("gl", "glg", "glg"),
    ("gu", "guj", "guj"),
    ("he", "heb", "heb"),
    ("hi", "hin", "hin"),
    ("hr", "hrv", "hrv"),
    ("hu", "hun", "hun"),
    ("hy", "hye", "arm"),
    ("id", "ind", "ind"),
    ("is", "isl", "ice"),
    ("it", "ita", "ita"),
    ("ja", "jpn", "jpn"),
    ("ka", "kat", "geo"),
    ("kk", "kaz", "kaz"),
    ("km", "khm", "khm"),
    ("kn", "kan", "kan"),
    ("ko", "kor", "kor"),
    ("ku", "kur", "kur"),
    ("la", "lat", "lat"),
    ("lb", "ltz", "ltz"),
    ("lo", "lao", "lao"),
    ("lt", "lit", "lit"),
    ("lv", "lav", "lav"),
    ("mk", "mkd", "mac"),
    ("ml", "mal", "mal"),
    ("mn", "mon", "mon"),
    ("mr", "mar", "mar"),
    ("ms", "msa", "may"),
    ("my", "mya", "bur"),
    ("nb", "nob", "nob"),
    ("ne", "nep", "nep"),
    ("nl", "nld", "dut"),
    ("nn", "nno", "nno"),
    ("no", "nor", "nor"),
    ("pa", "pan", "pan"),
    ("pl", "pol", "pol"),
    ("ps", "pus", "pus"),
    ("pt", "por", "por"),
    ("ro", "ron", "rum"),
    ("ru", "rus", "rus"),
    ("si", "sin", "sin"),
    ("sk", "slk", "slo"),
    ("sl", "slv", "slv"),
    ("so", "som", "som"),
    ("sq", "sqi", "alb"),
    ("sr", "srp", "srp"),
    ("sv", "swe", "swe"),
    ("sw", "swa", "swa"),
    ("ta", "tam", "tam"),
    ("te", "tel", "tel"),
    ("th", "tha", "tha"),
    ("tl", "tgl", "tgl"),
    ("tr", "tur", "tur"),
    ("uk", "ukr", "ukr"),
    ("ur", "urd", "urd"),
    ("uz", "uzb", "uzb"),
    ("vi", "vie", "vie"),
    ("yi", "yid", "yid"),
    ("zh", "zho", "chi"),
    ("zu", "zul", "zul"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn code(s: &str) -> Option<String> {
        Language::parse(s).map(|l| l.code())
    }

    #[test]
    fn both_forms_of_iso_639_2_read_as_one_language() {
        assert_eq!(code("jpn"), Some("ja".into()));
        assert_eq!(code("JA"), Some("ja".into()));
        assert_eq!(code("fre"), Some("fr".into()));
        assert_eq!(code("fra"), Some("fr".into()));
        assert_eq!(code("ger"), Some("de".into()));
        assert_eq!(code("chi"), Some("zh".into()));
        // A language without a two-letter code keeps its three letters.
        assert_eq!(code("yue"), Some("yue".into()));
    }

    #[test]
    fn regions_are_kept_and_codes_naming_no_language_are_unknown() {
        assert_eq!(code("pt-br"), Some("pt-BR".into()));
        assert_eq!(code("por_BR"), Some("pt-BR".into()));
        assert_eq!(code("es-419"), Some("es-419".into()));
        for nothing in [
            "",
            "und",
            "mul",
            "zxx",
            "e",
            "engl",
            "pt-",
            "pt-Brazil",
            "1a",
        ] {
            assert_eq!(code(nothing), None, "{nothing}");
        }
    }

    #[test]
    fn a_region_never_stands_in_for_another() {
        let want = |s: &str| Language::parse(s).unwrap();
        assert_eq!(want("pt-BR").answers(&want("pt-BR")), Some(2));
        assert_eq!(want("pt-BR").answers(&want("pt-PT")), None);
        assert_eq!(want("pt-BR").answers(&want("por")), Some(1));
        assert_eq!(want("pt").answers(&want("pt-PT")), Some(1));
        assert_eq!(want("en").answers(&want("eng")), Some(2));
        assert_eq!(want("en").answers(&want("fre")), None);
    }
}
