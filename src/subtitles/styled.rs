//! Whether an ASS/SSA track is typeset, so WebVTT, which keeps only the words, would lose
//! what it shows.

use std::collections::HashSet;

/// Whether an ASS/SSA script is styled rather than plain dialogue: a dialogue line places,
/// moves, clips, rotates, draws, animates or times karaoke (`\pos`, `\move`, `\an`, `\a`,
/// `\org`, `\clip`, `\iclip`, `\fr`, `\p1`, `\t`, `\k`), or the dialogue uses more than one
/// style. Comment lines and styles defined but never used don't count.
pub fn styled_ass(script: &str) -> bool {
    let mut columns: Option<(usize, usize)> = None;
    let mut in_events = false;
    let mut styles = HashSet::new();
    for line in script.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(format) = strip_key(line, "format") {
            columns = Some(event_columns(format));
            continue;
        }
        let Some(fields) = strip_key(line, "dialogue") else {
            continue;
        };
        // ASS's usual order, for a script without a Format line.
        let (style, count) = columns.unwrap_or((3, 10));
        let fields: Vec<&str> = fields.splitn(count, ',').collect();
        if fields.len() < count {
            continue;
        }
        // A leading `*` on a style name is a legacy marker, not part of the name.
        styles.insert(
            fields[style]
                .trim()
                .trim_start_matches('*')
                .to_ascii_lowercase(),
        );
        if styles.len() > 1 || fields[count - 1].split('{').skip(1).any(styled_block) {
            return true;
        }
    }
    false
}

/// The rest of `line` after `key:`, matched in any case.
fn strip_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (name, rest) = line.split_once(':')?;
    name.trim().eq_ignore_ascii_case(key).then_some(rest)
}

/// Where the Style column is in an Events Format line, and how many columns there are (the
/// last, Text, may hold commas).
fn event_columns(format: &str) -> (usize, usize) {
    let names: Vec<String> = format
        .split(',')
        .map(|n| n.trim().to_ascii_lowercase())
        .collect();
    let style = names.iter().position(|n| n == "style").unwrap_or(3);
    (style, names.len().max(style + 2))
}

/// Whether an override block (the text after its `{`) holds a tag that typesets.
fn styled_block(block: &str) -> bool {
    let block = block.split('}').next().unwrap_or_default();
    block.split('\\').skip(1).any(|tag| {
        let digit_after = |name: &str| {
            tag.strip_prefix(name)
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| c.is_ascii_digit())
        };
        let drawing = tag
            .strip_prefix('p')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| matches!(c, '1'..='9'));
        [
            "pos(", "move(", "org(", "clip(", "iclip(", "t(", "frx", "fry", "frz",
        ]
        .iter()
        .any(|name| tag.starts_with(name))
            || ["an", "a", "k", "K", "kf", "ko", "fr", "fr-"]
                .iter()
                .any(|name| digit_after(name))
            || drawing
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(events: &str) -> String {
        format!(
            "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Alignment\n\
             Style: Default,Arial,2\nStyle: Sign,Fancy,8\n\n[Events]\n\
             Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n{events}"
        )
    }

    fn line(style: &str, text: &str) -> String {
        format!("Dialogue: 0,0:00:00.00,0:00:01.00,{style},,0,0,0,,{text}\n")
    }

    #[test]
    fn plain_dialogue_isnt_styled() {
        let plain = script(&format!(
            "{}{}Comment: 0,0:00:00.00,0:00:01.00,Sign,,0,0,0,,{{\\pos(1,2)}}note\n",
            line("Default", "Hello, there \\N second line"),
            line(
                "*default",
                "{\\i1}Italic{\\i0}, {\\b1}bold{\\fad(100,100)\\c&H00FF00&\\alpha&H80&\\fs20\\fnArial\\pbo2\\p0}"
            ),
        ));
        assert!(!styled_ass(&plain), "{plain}");
        assert!(!styled_ass(""));
        assert!(!styled_ass(
            "1\n00:00:00,000 --> 00:00:01,000\n{\\pos(1,2)}Hi\n"
        ));
    }

    #[test]
    fn typesetting_tags_make_it_styled() {
        for text in [
            "{\\pos(160,40)}Sign",
            "{\\move(0,0,100,100)}Moving",
            "{\\an8}On top",
            "{\\a6}Legacy alignment",
            "{\\org(10,10)\\frz30}Turned",
            "{\\clip(0,0,10,10)}Clipped",
            "{\\iclip(m 0 0 l 10 10)}Clipped",
            "{\\fr-15}Turned",
            "{\\fry40}Turned",
            "{\\p1}m 0 0 l 100 0 100 100{\\p0}",
            "{\\t(0,500,\\fscx120)}Growing",
            "{\\k20}Ka{\\k30}ra",
            "{\\K20}Ka",
            "{\\kf40}oke",
            "{\\ko10}oke",
            "Text then {\\b1\\pos(1,2)}a sign",
        ] {
            assert!(styled_ass(&script(&line("Default", text))), "{text}");
        }
    }

    #[test]
    fn more_than_one_style_in_dialogue_is_styled() {
        let two = script(&format!(
            "{}{}",
            line("Default", "Hi"),
            line("Sign", "A sign")
        ));
        assert!(styled_ass(&two));
    }

    #[test]
    fn reads_the_format_line_for_where_style_and_text_are() {
        // SSA's own order, Marked first; and a script with only two columns.
        let ssa = "[Events]\nFormat: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
                   Dialogue: Marked=0,0:00:00.00,0:00:01.00,Default,,0,0,0,,{\\pos(1,2)}Sign\n";
        assert!(styled_ass(ssa));
        let short = "[Events]\nFormat: Style, Text\n\
                     Dialogue: Default,Hello, Sign\nDialogue: Default,Again\n";
        assert!(!styled_ass(short), "a comma in the text isn't a column");
        let short = "[Events]\nFormat: Style, Text\n\
                     Dialogue: Default,Hello\nDialogue: Sign,Again\n";
        assert!(styled_ass(short));
    }
}
