//! An entity MusicBrainz has merged into another: a lookup of the old id answers with a redirect
//! to the surviving one's (`/ws/2/release/<id>`). The redirect is never followed as given (it
//! can point at plain http, or anywhere); its new id is read from the address, which must be a
//! lookup of the same kind of entity on MusicBrainz, and the client asks for that id itself,
//! on its own address. Anything else is refused, and the error says where it pointed.

use super::*;

/// What one request got.
pub(super) enum Fetched {
    /// The answer's body.
    Body(String),
    /// A redirect: its status and the address it gave (the raw `Location`), if any.
    Moved(reqwest::StatusCode, Option<String>),
}

/// Where a redirect pointed, for an error or the log: scheme, host and path, never the query.
pub(super) fn described(request: &str, location: Option<&str>) -> String {
    let Some(url) = location.and_then(|l| resolved(request, l)) else {
        return "an address that couldn't be read".into();
    };
    let mut shown = format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        url.path()
    );
    if shown.len() > 200 {
        let cut = (0..=200)
            .rev()
            .find(|&i| shown.is_char_boundary(i))
            .unwrap_or(0);
        shown.truncate(cut);
        shown.push('…');
    }
    shown
}

/// The redirect's address, read against the request's (a `Location` may be relative).
fn resolved(request: &str, location: &str) -> Option<reqwest::Url> {
    reqwest::Url::parse(request).ok()?.join(location).ok()
}

/// The surviving id a redirect of a lookup of `entity` names: only when it points at
/// MusicBrainz (or the client's own host) and its path is exactly `/ws/2/<entity>/<id>`.
pub(super) fn moved_to(
    base: &str,
    request: &str,
    entity: &str,
    location: Option<&str>,
) -> Option<String> {
    let url = resolved(request, location?)?;
    let own = reqwest::Url::parse(base).ok();
    let host = url.host_str()?;
    if host != "musicbrainz.org" && own.as_ref().and_then(|b| b.host_str()) != Some(host) {
        return None;
    }
    match url.path_segments()?.collect::<Vec<_>>()[..] {
        ["ws", "2", kind, id] if kind == entity && crate::library::tags::is_mbid(id) => {
            Some(id.to_owned())
        }
        _ => None,
    }
}

impl Client {
    /// An entity by id (`query` asks for what comes with it): when MusicBrainz says it was
    /// merged into another, the survivor, asked for once on the client's own address.
    pub(super) async fn lookup(
        &self,
        entity: &str,
        id: &str,
        query: &str,
    ) -> Result<String, Error> {
        let url = format!("{}/{entity}/{id}?{query}", self.base);
        let (status, location) = match self.fetch(&url).await? {
            Fetched::Body(body) => return Ok(body),
            Fetched::Moved(status, location) => (status, location),
        };
        let Some(survivor) = moved_to(&self.base, &url, entity, location.as_deref()) else {
            return Err(refused(status, &url, location.as_deref()));
        };
        tracing::info!(
            entity,
            from = id,
            to = survivor,
            "MusicBrainz merged it; asking for the one it joined"
        );
        let again = format!("{}/{entity}/{survivor}?{query}", self.base);
        match self.fetch(&again).await? {
            Fetched::Body(body) => Ok(body),
            Fetched::Moved(status, location) => Err(Error::Failed(format!(
                "answered {status} again, to {}; one move is followed",
                described(&again, location.as_deref())
            ))),
        }
    }
}

/// A redirect not followed, saying where it pointed.
pub(super) fn refused(status: reqwest::StatusCode, request: &str, location: Option<&str>) -> Error {
    let to = described(request, location);
    tracing::warn!(%status, to, "MusicBrainz redirected elsewhere; not followed");
    Error::Failed(format!("answered {status}, to {to}, which isn't followed"))
}
