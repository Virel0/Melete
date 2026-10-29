//! Reading a Torznab feed: its capabilities, its items with their attributes and units, its errors,
//! and RFC 2822 dates.

use super::*;

/// A Torznab `<error code=… description=…/>`. Code 100 is a wrong API key.
pub(super) fn torznab_error(e: &BytesStart) -> Error {
    let code = attribute(e, "code").unwrap_or_default();
    if code == "100" {
        return Error::Rejected;
    }
    // Shown to admins in an error; an indexer could send megabytes.
    let description: String = attribute(e, "description")
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    Error::Indexer { code, description }
}

pub(super) fn attribute(e: &BytesStart, name: &str) -> Option<String> {
    let attribute = e.try_get_attribute(name).ok()??;
    let value = attribute.normalized_value(XmlVersion::Implicit1_0).ok()?;
    Some(value.into_owned())
}

pub(super) fn malformed(err: impl fmt::Display) -> Error {
    Error::Malformed(err.to_string())
}

/// Reads a Torznab `t=caps` reply.
///
/// # Errors
///
/// [`Error::Indexer`] or [`Error::Rejected`] when the reply is a Torznab error, and
/// [`Error::Malformed`] when it isn't XML or has no `<caps>`.
pub fn parse_caps(xml: &str) -> Result<Caps, Error> {
    let mut reader = Reader::from_str(xml);
    let mut caps = Caps::default();
    let mut seen_caps = false;
    loop {
        match reader.read_event().map_err(malformed)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "caps" => seen_caps = true,
                "error" => return Err(torznab_error(&e)),
                "category" | "subcat" => {
                    if let Some(id) = attribute(&e, "id").and_then(|id| id.parse::<i64>().ok())
                        && id < 100_000
                    {
                        caps.categories.push(id);
                    }
                }
                kind @ ("movie-search" | "tv-search") => {
                    let params = match attribute(&e, "available").as_deref() {
                        Some("yes") => attribute(&e, "supportedParams")
                            .unwrap_or_default()
                            .split(',')
                            .map(|p| p.trim().to_owned())
                            .filter(|p| !p.is_empty())
                            .collect(),
                        _ => Vec::new(),
                    };
                    match kind {
                        "movie-search" => caps.movie_search = params,
                        _ => caps.tv_search = params,
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_caps {
        return Err(Error::Malformed("no <caps>".into()));
    }
    caps.categories.sort_unstable();
    caps.categories.dedup();
    Ok(caps)
}

/// Reads a Torznab search reply, or any RSS feed of releases, into its items.
///
/// # Errors
///
/// [`Error::Indexer`] or [`Error::Rejected`] when the reply is a Torznab error, and
/// [`Error::Malformed`] when it isn't XML or has no `<rss>`.
pub fn parse_items(xml: &str) -> Result<Vec<Item>, Error> {
    let mut reader = Reader::from_str(xml);
    let mut items = Vec::new();
    let mut seen_rss = false;
    let mut item: Option<Item> = None;
    // The element whose text is being read, and the text so far (entities arrive apart).
    let mut field: Option<String> = None;
    let mut text = String::new();
    loop {
        match reader.read_event().map_err(malformed)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "rss" => seen_rss = true,
                "error" => return Err(torznab_error(&e)),
                "item" => item = Some(Item::default()),
                name => {
                    if let Some(item) = item.as_mut() {
                        // <enclosure ...></enclosure> carries its data as attributes too.
                        apply_attributes(item, &e);
                        field = Some(name.to_owned());
                        text.clear();
                    }
                }
            },
            Event::Empty(e) => match (e.local_name().as_ref(), item.as_mut()) {
                ("rss", _) => seen_rss = true,
                ("error", _) => return Err(torznab_error(&e)),
                (_, Some(item)) => apply_attributes(item, &e),
                _ => {}
            },
            Event::Text(t) if field.is_some() => text.push_str(&t.xml10_content()),
            Event::CData(c) if field.is_some() => text.push_str(&c.xml10_content()),
            Event::GeneralRef(r) if field.is_some() => {
                match r.resolve_char_ref().map_err(malformed)? {
                    Some(c) => text.push(c),
                    None => {
                        let name = r.xml10_content();
                        let resolved = resolve_predefined_entity(&name)
                            .ok_or_else(|| Error::Malformed(format!("unknown entity &{name};")))?;
                        text.push_str(resolved);
                    }
                }
            }
            Event::End(e) => {
                if e.local_name().as_ref() == "item" {
                    if let Some(done) = item.take()
                        && !done.title.is_empty()
                    {
                        items.push(done);
                    }
                } else if let (Some(name), Some(item)) = (field.take(), item.as_mut()) {
                    apply_field(item, &name, text.trim());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_rss {
        return Err(Error::Malformed("no <rss>".into()));
    }
    if item.is_some() {
        return Err(Error::Malformed("the reply ends inside an item".into()));
    }
    Ok(items)
}

/// The data `<enclosure>` and `<torznab:attr>` carry as attributes.
pub(super) fn apply_attributes(item: &mut Item, e: &BytesStart) {
    match e.local_name().as_ref() {
        "enclosure" => {
            if let Some(url) = attribute(e, "url") {
                item.download_url = Some(url);
            }
            if item.size.is_none() {
                item.size = attribute(e, "length").and_then(|l| l.parse().ok());
            }
        }
        "attr" => {
            if let (Some(name), Some(value)) = (attribute(e, "name"), attribute(e, "value")) {
                apply_attr(item, &name, &value);
            }
        }
        _ => {}
    }
}

pub(super) fn apply_field(item: &mut Item, name: &str, text: &str) {
    let owned = || (!text.is_empty()).then(|| text.to_owned());
    match name {
        "title" => item.title = text.to_owned(),
        "guid" => item.guid = owned(),
        // The enclosure's URL, when there is one, is the download.
        "link" if item.download_url.is_none() => item.download_url = owned(),
        "comments" => item.info_url = owned(),
        "pubDate" => item.published = rfc2822(text),
        "size" => item.size = text.parse().ok().or_else(|| with_unit(text)),
        // Nyaa's own: <nyaa:seeders>, <nyaa:leechers>, <nyaa:infoHash>.
        "seeders" => item.seeders = text.parse().ok(),
        "leechers" => item.peers = text.parse().ok(),
        "infoHash" if !text.is_empty() => item.infohash = Some(text.to_ascii_lowercase()),
        "category" => {
            if let Ok(id) = text.parse::<i64>()
                && id < 100_000
                && !item.categories.contains(&id)
            {
                item.categories.push(id);
            }
        }
        _ => {}
    }
}

/// "1.2 GiB" (Nyaa's sizes) as bytes.
pub(super) fn with_unit(text: &str) -> Option<u64> {
    let (number, unit) = text.split_once(' ')?;
    let number: f64 = number.parse().ok()?;
    let factor: f64 = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((number * factor).round() as u64)
}

pub(super) fn apply_attr(item: &mut Item, name: &str, value: &str) {
    let mut flag = |flag: &str| {
        if !item.flags.iter().any(|f| f == flag) {
            item.flags.push(flag.to_owned());
        }
    };
    match (name, value) {
        ("downloadvolumefactor", "0") => flag("freeleech"),
        ("downloadvolumefactor", "0.25") => flag("freeleech75"),
        ("downloadvolumefactor", "0.5") => flag("halfleech"),
        ("downloadvolumefactor", "0.75") => flag("freeleech25"),
        ("uploadvolumefactor", "2") => flag("double_upload"),
        ("tag", tag @ ("freeleech" | "internal" | "scene" | "nuked")) => flag(tag),
        ("seeders", seeders) => item.seeders = seeders.parse().ok(),
        ("peers", peers) => item.peers = peers.parse().ok(),
        ("infohash", hash) if !hash.is_empty() => item.infohash = Some(hash.to_ascii_lowercase()),
        ("size", size) if item.size.is_none() => item.size = size.parse().ok(),
        // 0 means unknown.
        ("imdb" | "imdbid", id) => {
            let digits = id.trim_start_matches("tt").trim_start_matches('0');
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                item.imdb_id = Some(digits.to_owned());
            }
        }
        ("tvdbid", id) => item.tvdb_id = id.parse().ok().filter(|id| *id > 0),
        ("tmdbid", id) => item.tmdb_id = id.parse().ok().filter(|id| *id > 0),
        ("category", category) => {
            if let Ok(id) = category.parse::<i64>()
                && id < 100_000
                && !item.categories.contains(&id)
            {
                item.categories.push(id);
            }
        }
        _ => {}
    }
}

/// "Sat, 24 Oct 2026 10:00:00 +0000" as Unix seconds.
pub(super) fn rfc2822(text: &str) -> Option<i64> {
    let text = text.split_once(',').map_or(text, |(_, rest)| rest).trim();
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [day, month, year, time, zone] = parts.as_slice() else {
        return None;
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = months.iter().position(|m| m.eq_ignore_ascii_case(month))? as i64 + 1;
    let (day, year): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let hms: Vec<i64> = time
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (hour, minute, second) = match hms.as_slice() {
        [h, m] => (*h, *m, 0),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    let offset = match zone.as_bytes() {
        [sign @ (b'+' | b'-'), digits @ ..] if digits.len() == 4 => {
            let n: i64 = std::str::from_utf8(digits).ok()?.parse().ok()?;
            let minutes = n / 100 * 60 + n % 100;
            if *sign == b'-' { -minutes } else { minutes }
        }
        _ if ["GMT", "UT", "UTC", "Z"].contains(zone) => 0,
        _ => return None,
    };
    if !(1900..=9999).contains(&year)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days since 1970-01-01, from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset * 60)
}
