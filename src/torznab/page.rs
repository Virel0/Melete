//! What a web page that came back in a feed's place says: a site that has turned its feed off,
//! or wants a sign-in, answers with one. Only an HTML document counts (a feed starts with XML),
//! and only what it shows as its message: its first heading, else its title.

/// The most of a page's message said, in characters.
const MOST: usize = 120;

/// What an HTML page says, as plain text; None when `body` isn't an HTML page or says nothing.
pub(super) fn says(body: &str) -> Option<String> {
    let start = body.trim_start();
    let head: String = start
        .chars()
        .take(64)
        .collect::<String>()
        .to_ascii_lowercase();
    if !(head.starts_with("<!doctype html") || head.starts_with("<html")) {
        return None;
    }
    let lower = start.to_ascii_lowercase();
    ["h1", "h2", "h3", "title"]
        .iter()
        .find_map(|tag| inside(start, &lower, tag))
}

/// The text inside the first `<tag …>…</tag>` of `page` (`lower` being it in lower case), with
/// its markup and entities undone; None when there's none or it's blank.
fn inside(page: &str, lower: &str, tag: &str) -> Option<String> {
    let open = lower.find(&format!("<{tag}"))?;
    let after = open + lower[open..].find('>')? + 1;
    let close = after + lower[after..].find(&format!("</{tag}"))?;
    let mut text = String::new();
    let mut in_tag = false;
    for c in page[after..close].chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let said = words.join(" ");
    Some(if said.chars().count() > MOST {
        format!("{}…", said.chars().take(MOST).collect::<String>())
    } else {
        said
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pages_heading_else_its_title_is_what_it_says() {
        let disabled = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "x"><html><head><title>Error</title><link rel="stylesheet" href="s.css" /></head><body><div class="errorbox"><h2>Feed disabled</h2><div class="message">Feed disabled</div></div></body></html>"#;
        assert_eq!(says(disabled).as_deref(), Some("Feed disabled"));
        let titled =
            "<html><head><title>Sign in &amp; search</title></head><body><p>x</p></body></html>";
        assert_eq!(says(titled).as_deref(), Some("Sign in & search"));
        let nested = "<html><body><h1>  Down <b>for</b>\n maintenance </h1></body></html>";
        assert_eq!(says(nested).as_deref(), Some("Down for maintenance"));
        let long = format!("<html><h1>{}</h1></html>", "word ".repeat(60));
        assert!(says(&long).unwrap().ends_with('…'));
        assert_eq!(says(&long).unwrap().chars().count(), MOST + 1);
    }

    #[test]
    fn feeds_and_pages_with_nothing_to_say_are_left_alone() {
        assert_eq!(
            says(r#"<?xml version="1.0"?><rss><channel><title>Feed</title></channel></rss>"#),
            None
        );
        assert_eq!(says("<caps><server title=\"x\"/></caps>"), None);
        assert_eq!(
            says("<error code=\"100\" description=\"Invalid API Key\"/>"),
            None
        );
        assert_eq!(says("<html>login</html>"), None);
        assert_eq!(says("<html><title>   </title></html>"), None);
    }
}
