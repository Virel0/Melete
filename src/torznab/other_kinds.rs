//! The indexers that are not Torznab feeds, read in their own forms: The Pirate Bay through apibay
//! JSON, and Uindex pages (through FlareSolverr).

use super::*;

/// The Pirate Bay's video categories, with Torznab's for them.
pub(super) const APIBAY_CATEGORIES: [(i64, i64); 6] = [
    (201, 2000),
    (207, 2040),
    (211, 2045),
    (205, 5000),
    (208, 5040),
    (212, 5045),
];

/// Its music categories, read only when music is what's asked for.
pub(super) const APIBAY_AUDIO_CATEGORIES: [(i64, i64); 2] = [(101, 3000), (104, 3040)];

/// Trackers The Pirate Bay's own magnet links name.
pub(super) const APIBAY_TRACKERS: [&str; 5] = [
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
];

/// A search's title without the numbers it was given: episodes ("Frieren S01E05", "Show
/// S02") or a year ("Suzume 2022"), which fansub names seldom carry.
pub(super) fn strip_numbers(query: &str) -> &str {
    match query.rsplit_once(' ') {
        Some((title, last))
            if last.len() >= 3
                && last.starts_with(['S', 's'])
                && last[1..]
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == 'E' || c == 'e') =>
        {
            title
        }
        Some((title, last))
            if last.len() == 4 && last.bytes().all(|b| b.is_ascii_digit()) && !title.is_empty() =>
        {
            title
        }
        _ => query,
    }
}

/// Searches Uindex through FlareSolverr; without a query, its newest. Only `wanted`'s kinds of
/// release come back.
pub(super) async fn search_uindex(
    flaresolverr: &crate::flaresolverr::Client,
    base: &str,
    query: Option<&str>,
    wanted: &[i64],
) -> Result<Vec<Item>, Error> {
    let mut url = reqwest::Url::parse(&format!("{}/search.php", base.trim_end_matches('/')))
        .map_err(|_| Error::Unreachable("the indexer's URL isn't one".into()))?;
    url.query_pairs_mut()
        .append_pair("search", query.unwrap_or_default())
        .append_pair("c", "0");
    let html = flaresolverr
        .get(url.as_str())
        .await
        .map_err(Error::Unreachable)?;
    if html.len() > MAX_RESPONSE_BYTES {
        return Err(Error::TooLarge);
    }
    let mut items = parse_uindex(&html, crate::db::unix_now())?;
    items.retain(|i| {
        i.categories
            .iter()
            .any(|c| wanted.iter().any(|w| w / 1000 == c / 1000))
    });
    Ok(items)
}

/// Uindex's results, read as Prowlarr's definition reads them: each row of `table.sr-table`
/// with a magnet link. Its categories 1 (movies), 2 (TV) and 7 (anime) are kept. `now` dates
/// "1.5 years ago".
pub fn parse_uindex(html: &str, now: i64) -> Result<Vec<Item>, Error> {
    use scraper::{Html, Selector};
    // Only called with the selectors written out below, which the tests parse.
    let selector = |css: &str| Selector::parse(css).expect("a valid selector");
    let document = Html::parse_document(html);
    let (table, rows) = (selector("table.sr-table"), selector("tbody > tr"));
    let (magnet, details, category) = (
        selector(r#"a[href^="magnet:?xt="]"#),
        selector(r#"a[href^="/details.php?id="]"#),
        selector(r#"a[href^="/search.php?c="]"#),
    );
    let cell = |name: &str| selector(&format!("td.sr-col-{name}"));
    let (size, uploaded, seeders, leechers) = (
        cell("size"),
        cell("uploaded"),
        cell("seeders"),
        cell("leechers"),
    );
    let Some(table) = document.select(&table).next() else {
        // A page without the results table isn't an empty search: something changed.
        return Err(Error::Malformed("no results table on Uindex's page".into()));
    };
    let text = |row: &scraper::ElementRef, which: &Selector| -> String {
        row.select(which)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_owned())
            .unwrap_or_default()
    };
    let mut items = Vec::new();
    for row in table.select(&rows) {
        let Some(link) = row
            .select(&magnet)
            .next()
            .and_then(|a| a.value().attr("href"))
        else {
            continue;
        };
        let Some(hash) = crate::deluge::magnet_hash(link) else {
            continue;
        };
        let Some(named) = row.select(&details).next() else {
            continue;
        };
        let title = named
            .value()
            .attr("title")
            .map(str::to_owned)
            .unwrap_or_else(|| named.text().collect::<String>())
            .trim()
            .to_owned();
        let id = named
            .value()
            .attr("href")
            .and_then(|h| h.split("id=").nth(1))
            .unwrap_or_default();
        let torznab = match row
            .select(&category)
            .next()
            .and_then(|a| a.value().attr("href"))
            .and_then(|h| h.split("c=").nth(1))
        {
            Some("1") => 2000,
            Some("2") => 5000,
            Some("7") => 5070,
            _ => continue,
        };
        if title.is_empty() {
            continue;
        }
        items.push(Item {
            title,
            guid: Some(format!("uindex:{id}")),
            download_url: Some(link.to_owned()),
            size: with_decimal_unit(&text(&row, &size)),
            seeders: text(&row, &seeders).parse().ok(),
            peers: text(&row, &leechers).parse().ok(),
            published: ago(&text(&row, &uploaded), now),
            categories: vec![torznab],
            infohash: Some(hash),
            ..Item::default()
        });
    }
    Ok(items)
}

/// "34.26 GB" (Uindex's sizes, counted in 1024s) as bytes.
pub(super) fn with_decimal_unit(text: &str) -> Option<u64> {
    let (number, unit) = text.split_once(' ')?;
    let binary = match unit {
        "B" => "B",
        "KB" => "KiB",
        "MB" => "MiB",
        "GB" => "GiB",
        "TB" => "TiB",
        _ => return None,
    };
    with_unit(&format!("{number} {binary}"))
}

/// "1.5 years ago", "3 hours ago" as Unix seconds, roughly.
pub(super) fn ago(text: &str, now: i64) -> Option<i64> {
    let mut words = text.split_whitespace();
    let number: f64 = match words.next()? {
        "a" | "an" => 1.0,
        n => n.parse().ok()?,
    };
    let unit = match words.next()?.trim_end_matches('s') {
        "second" => 1.0,
        "minute" => 60.0,
        "hour" => 3600.0,
        "day" => 86_400.0,
        "week" => 7.0 * 86_400.0,
        "month" => 30.0 * 86_400.0,
        "year" => 365.0 * 86_400.0,
        _ => return None,
    };
    (words.next()? == "ago").then(|| now - (number * unit) as i64)
}

/// The Pirate Bay's results: numbers come as strings from searches, as numbers from its lists.
/// "No results returned" is a row with id 0. Only video categories are kept.
pub fn parse_apibay(json: &str) -> Result<Vec<Item>, Error> {
    parse_apibay_in(json, &APIBAY_CATEGORIES)
}

/// Its results in `known` categories, with Torznab's categories for them.
pub(super) fn parse_apibay_in(json: &str, known: &[(i64, i64)]) -> Result<Vec<Item>, Error> {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(json).map_err(|err| Error::Malformed(err.to_string()))?;
    let number =
        |v: &serde_json::Value| -> Option<i64> { v.as_i64().or_else(|| v.as_str()?.parse().ok()) };
    let mut items = Vec::new();
    for row in &rows {
        let (Some(id), Some(name), Some(hash)) = (
            number(&row["id"]),
            row["name"].as_str(),
            row["info_hash"].as_str(),
        ) else {
            continue;
        };
        let hash = hash.to_ascii_lowercase();
        let plain_hash = hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        let category = number(&row["category"]).unwrap_or(0);
        let Some(&(_, torznab)) = known.iter().find(|(c, _)| *c == category) else {
            continue;
        };
        if id == 0 || !plain_hash {
            continue;
        }
        // xt as magnet links write it, unencoded; the name and trackers encoded.
        let encode = |text: &str| -> String {
            text.bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        (b as char).to_string()
                    }
                    _ => format!("%{b:02X}"),
                })
                .collect()
        };
        let mut magnet = format!("magnet:?xt=urn:btih:{hash}&dn={}", encode(name));
        for tracker in APIBAY_TRACKERS {
            magnet.push_str(&format!("&tr={}", encode(tracker)));
        }
        let mut item = Item {
            title: name.to_owned(),
            guid: Some(format!("apibay:{id}")),
            download_url: Some(magnet),
            published: number(&row["added"]),
            size: number(&row["size"]).and_then(|s| u64::try_from(s).ok()),
            seeders: number(&row["seeders"]).and_then(|s| u32::try_from(s).ok()),
            peers: number(&row["leechers"]).and_then(|s| u32::try_from(s).ok()),
            categories: vec![torznab],
            infohash: Some(hash),
            ..Item::default()
        };
        if let Some(imdb) = row["imdb"].as_str() {
            apply_attr(&mut item, "imdb", imdb);
        }
        items.push(item);
    }
    Ok(items)
}
