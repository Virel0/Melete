//! Mixes by description: a mood as smart-playlist rules in Aoide's own format (the JSON a
//! playlist's `smartRules` holds), with the names the description mentioned. As with music
//! search the model writes rules and never picks tracks; the apps run the rules against the
//! library and the listening history.
//!
//! **A field and an operator that don't go together are refused, never kept.** Such a rule
//! matches nothing, and a smart playlist that's silently empty looks exactly like one whose
//! rules have no hits; a refused rule comes back with its reason instead.

use super::music_search::vocabulary;
use super::*;

/// What a field holds, which decides the operators it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    Date,
    Flag,
}

/// Each field as the rules name it. `first_played`, `last_played`, `play_count` and
/// `skip_count` come from the listening history the apps keep.
const FIELDS: [(&str, Kind); 12] = [
    ("album", Kind::Text),
    ("album_artist", Kind::Text),
    ("artist", Kind::Text),
    ("duration_ms", Kind::Number),
    ("first_played", Kind::Date),
    ("genre", Kind::Text),
    ("last_played", Kind::Date),
    ("liked", Kind::Flag),
    ("play_count", Kind::Number),
    ("skip_count", Kind::Number),
    ("title", Kind::Text),
    ("year", Kind::Number),
];

/// The camelCase spellings the apps also read, written back as the rules spell them.
const ALIASES: [(&str, &str); 6] = [
    ("albumArtist", "album_artist"),
    ("durationMs", "duration_ms"),
    ("firstPlayed", "first_played"),
    ("lastPlayed", "last_played"),
    ("playCount", "play_count"),
    ("skipCount", "skip_count"),
];

fn operators(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Text => &["is", "isNot", "contains", "doesNotContain", "beginsWith"],
        Kind::Number => &["is", "isNot", "greaterThan", "lessThan"],
        Kind::Date => &["before", "after", "inTheLast", "notInTheLast"],
        Kind::Flag => &["is", "isNot"],
    }
}

/// What a description became.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mix {
    /// Artists, albums, bands, games, films or shows it named, as the model spelled them: the
    /// half of a description a rule can't say ("Helldivers 2 session music").
    pub names: Vec<String>,
    /// The rule set, `{match, limit, rules}` (limit null for no cap), with only rules the apps can evaluate; none when
    /// it asked for any of no usable rules, which could only ever be empty.
    pub rules: Option<Value>,
    /// Each rule left out, and why.
    pub rejected: Vec<String>,
}

impl Client {
    /// `description` as rules over a library with `genres`.
    pub async fn music_mix(&self, description: &str, genres: &[String]) -> Result<Mix, Failure> {
        let answer = self.complete(&mix_prompt(genres), description).await?;
        parse_mix(&answer).ok_or_else(|| {
            Failure::Other("the model's answer wasn't the rules asked for".to_owned())
        })
    }
}

/// The instructions, carrying the library's genres; the mood goes as the user's message.
fn mix_prompt(genres: &[String]) -> String {
    format!(
        "You turn a description of a mood into smart playlist rules. You never choose songs; \
         the rules select them.\n\n\
         Answer with JSON only, no other text:\n\
         {{\"match\": \"all\"|\"any\", \"limit\": number, \"rules\": [{{\"field\": ..., \
         \"op\": ..., \"value\": ...}}], \"names\": [string]}}\n\n\
         When the description names a particular artist, album, band, game, film or show, put \
         that name in \"names\" exactly as written; the library will be searched for it. Then \
         ALSO describe the mood that thing implies, using genres from the list: a war game or \
         an action film implies driving genres (rock, metal, electronic, soundtrack); a study \
         session, a farming or building game implies calm ones (lo-fi, ambient, jazz, \
         classical, folk). Someone naming a game wants music that fits playing it, not only \
         its soundtrack. Prefer two or three genres to one. \"names\" is [] when the \
         description names nothing.\n\n\
         Fields and the operators each accepts:\n\
         text: title, artist, album, album_artist, genre; is, isNot, contains, \
         doesNotContain, beginsWith\n\
         number: year, duration_ms, play_count, skip_count; is, isNot, greaterThan, lessThan\n\
         date: last_played, first_played; before, after, inTheLast, notInTheLast; value is a \
         span like \"-30d\"\n\
         flag: liked; is, isNot; value is true or false\n\n\
         Use the listening history (play_count, skip_count, last_played) to express mood: \
         \"something I have not heard in ages\" is last_played notInTheLast \"-6m\"; \"my \
         favourites\" is liked is true or play_count greaterThan a few.\n\n\
         genre values MUST come from this library, spelled exactly as given. Set limit to a \
         sensible mix length, usually 25 to 50.\n\n\
         This library's genres: {}",
        vocabulary(genres, "(none reported)"),
    )
}

/// The rules in a model's answer, the names it read, and why each rule left out was. None
/// without a JSON object.
fn parse_mix(answer: &str) -> Option<Mix> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    let value: Value = serde_json::from_str(answer.get(start..=end)?).ok()?;
    let mut mix = Mix {
        names: names(&value["names"]),
        ..Mix::default()
    };
    let mut rules = Vec::new();
    for rule in value["rules"].as_array().into_iter().flatten() {
        match read_rule(rule) {
            Ok(rule) => rules.push(rule),
            Err(why) => mix.rejected.push(why),
        }
    }
    let any = value["match"] == "any";
    if any && rules.is_empty() {
        mix.rejected.push("No usable rules.".to_owned());
        return Some(mix);
    }
    let limit = whole(&value["limit"]).filter(|n| (1..=i64::from(u32::MAX)).contains(n));
    mix.rules = Some(json!({
        "match": if any { "any" } else { "all" },
        "limit": limit,
        "rules": rules,
    }));
    Some(mix)
}

/// Strings of two characters or more, trimmed, each once; anything else is ignored, since a
/// stray number is no reason to lose the mix.
fn names(value: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let name = name.trim();
        if name.chars().count() >= 2 && !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// One rule as the apps keep it, or why it can't be.
fn read_rule(rule: &Value) -> Result<Value, String> {
    let said = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_owned);
    let named = rule["field"].as_str().unwrap_or_default();
    let named = ALIASES
        .iter()
        .find(|(alias, _)| *alias == named)
        .map_or(named, |(_, name)| name);
    let Some(&(field, kind)) = FIELDS.iter().find(|(name, _)| *name == named) else {
        return Err(format!("Unknown field {}", said(&rule["field"])));
    };
    let op = said(&rule["op"]);
    if !operators(kind).contains(&op.as_str()) {
        return Err(format!("{op} does not apply to {field}"));
    }
    let value = &rule["value"];
    let value = match kind {
        Kind::Text => value.as_str().map(|_| value.clone()).ok_or("needs text"),
        Kind::Number => whole(value).map(Value::from).ok_or("needs a whole number"),
        Kind::Flag => value
            .as_bool()
            .map(Value::from)
            .ok_or("needs true or false"),
        Kind::Date => return date(field, &op, value),
    }
    .map_err(|needs| format!("{field} {needs}"))?;
    Ok(json!({ "field": field, "op": op, "value": value }))
}

/// A date rule: a span back from now (`-30d`, `6m`, `2y`), or for `before` and `after` a
/// millisecond since 1970 too; a bare number for a span would be in unstated units.
fn date(field: &str, op: &str, value: &Value) -> Result<Value, String> {
    if let Some(text) = value.as_str() {
        if !is_span(text) {
            return Err(format!("{text} is not a span like -30d"));
        }
    } else if matches!(op, "inTheLast" | "notInTheLast") {
        return Err(format!("{op} needs a span like -30d"));
    } else if whole(value).is_none() {
        return Err(format!("{field} needs a time"));
    }
    let value = whole(value).map_or_else(|| value.clone(), Value::from);
    Ok(json!({ "field": field, "op": op, "value": value }))
}

fn is_span(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    match digits.char_indices().last() {
        Some((at, 'd' | 'w' | 'm' | 'y')) => {
            at > 0 && digits[..at].chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// A whole number, `3` and `3.0` alike, never `2.5` or `"3"`: every number field counts
/// something, and 2.5 would mean deciding which way to round a bound.
fn whole(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        let f = value.as_f64()?;
        (f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0).then_some(f as i64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix(answer: Value) -> Mix {
        parse_mix(&answer.to_string()).unwrap()
    }

    #[test]
    fn the_prompt_carries_the_librarys_genres_and_asks_for_names() {
        let prompt = mix_prompt(&["Rock".into(), "Lo-Fi".into()]);
        assert!(prompt.contains("This library's genres: Rock, Lo-Fi"));
        assert!(prompt.contains("put that name in \"names\" exactly as written"));
        assert!(prompt.contains("You never choose songs"));
        assert!(mix_prompt(&[]).contains("(none reported)"));
    }

    #[test]
    fn keeps_the_rules_that_work_and_says_why_it_dropped_the_rest() {
        let found = mix(
            json!({ "match": "all", "limit": 40.0, "names": ["Helldivers 2"],
            "rules": [
                { "field": "genre", "op": "is", "value": "Rock" },
                { "field": "lastPlayed", "op": "notInTheLast", "value": "-6m" },
                { "field": "year", "op": "greaterThan", "value": 1989.0 },
                { "field": "liked", "op": "is", "value": true },
                { "field": "last_played", "op": "before", "value": 1700000000000_i64 },
                { "field": "duration_ms", "op": "beginsWith", "value": 3 },
                { "field": "year", "op": "is", "value": 2.5 },
                { "field": "liked", "op": "is", "value": "yes" },
                { "field": "title", "op": "is", "value": 7 },
                { "field": "last_played", "op": "inTheLast", "value": 30 },
                { "field": "first_played", "op": "after", "value": "soon" },
                { "field": "mood", "op": "is", "value": "chill" },
                { "op": "is", "value": "x" }
            ] }),
        );
        assert_eq!(found.names, ["Helldivers 2"]);
        assert_eq!(
            found.rules.unwrap(),
            json!({ "match": "all", "limit": 40, "rules": [
                { "field": "genre", "op": "is", "value": "Rock" },
                { "field": "last_played", "op": "notInTheLast", "value": "-6m" },
                { "field": "year", "op": "greaterThan", "value": 1989 },
                { "field": "liked", "op": "is", "value": true },
                { "field": "last_played", "op": "before", "value": 1700000000000_i64 },
            ] })
        );
        assert_eq!(
            found.rejected,
            [
                "beginsWith does not apply to duration_ms",
                "year needs a whole number",
                "liked needs true or false",
                "title needs text",
                "inTheLast needs a span like -30d",
                "soon is not a span like -30d",
                "Unknown field mood",
                "Unknown field null",
            ]
        );
    }

    #[test]
    fn any_of_nothing_is_refused_and_all_of_nothing_is_the_whole_library() {
        let found = mix(json!({ "match": "any", "names": ["Muse"],
            "rules": [{ "field": "year", "op": "after", "value": 1990 }] }));
        assert_eq!(found.rules, None);
        assert_eq!(found.names, ["Muse"]);
        assert_eq!(
            found.rejected,
            ["after does not apply to year", "No usable rules."]
        );
        let found = mix(json!({ "limit": -5 }));
        assert_eq!(
            found.rules.unwrap(),
            json!({ "match": "all", "limit": null, "rules": [] })
        );
        assert!(found.names.is_empty() && found.rejected.is_empty());
    }

    #[test]
    fn names_are_strings_of_two_characters_or_more_each_once() {
        let found = mix(json!({ "names": [" Muse ", "Muse", "x", "", 3, "Deftones"] }));
        assert_eq!(found.names, ["Muse", "Deftones"]);
        assert!(mix(json!({ "names": "Muse" })).names.is_empty());
    }

    #[test]
    fn rules_are_read_from_whatever_the_model_wraps_them_in() {
        let fenced = "Here:\n```json\n{\"rules\": [{\"field\": \"genre\", \"op\": \"contains\", \"value\": \"Jazz\"}]}\n```";
        assert_eq!(
            parse_mix(fenced).unwrap().rules.unwrap()["rules"][0]["value"],
            "Jazz"
        );
        assert!(parse_mix("Sorry, I can't.").is_none());
        assert!(is_span("-30d") && is_span("6m") && is_span("2y") && is_span("-1w"));
        assert!(!is_span("-d") && !is_span("30") && !is_span("3x") && !is_span("-3.5d"));
    }
}
