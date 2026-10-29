//! LRC text as lines in time order, with word times and other tags dropped.

use super::*;

/// "01:02.34" (or "1:02", "01:02:340") as seconds.
pub(super) fn timestamp(tag: &str) -> Option<f64> {
    let (minutes, rest) = tag.split_once(':')?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    // Some files write the hundredths after a second colon.
    let seconds: f64 = rest.trim().replacen(':', ".", 1).parse().ok()?;
    (seconds.is_finite() && (0.0..60.0).contains(&seconds))
        .then(|| f64::from(minutes) * 60.0 + seconds)
}

/// An enhanced-LRC line without its word timings ("<00:01.20>").
pub(super) fn without_word_times(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let Some(close) = rest[open..].find('>') else {
            break;
        };
        out.push_str(&rest[..open]);
        let inner = &rest[open + 1..open + close];
        if timestamp(inner).is_none() {
            out.push_str(&rest[open..=open + close]);
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// LRC as lines in time order: a line with several times is repeated at each, the `offset`
/// tag is applied, other tags are dropped.
pub fn lrc_lines(text: &str) -> Vec<Line> {
    let mut offset = 0.0;
    let mut lines: Vec<(f64, String)> = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some(close) = tag.find(']') else {
                break;
            };
            let inside = &tag[..close];
            match timestamp(inside) {
                Some(time) => times.push(time),
                None => {
                    if let Some(("offset", ms)) = inside.split_once(':').map(|(k, v)| (k.trim(), v))
                    {
                        offset = ms.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
                    }
                }
            }
            rest = &tag[close + 1..];
        }
        let words = without_word_times(rest);
        for time in times {
            lines.push((time, words.clone()));
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines
        .into_iter()
        .map(|(time, text)| Line {
            // A positive offset shows the lyrics sooner.
            start: Some(((time - offset).max(0.0) * 1000.0).round() / 1000.0),
            text,
        })
        .collect()
}
