//! The requests: checking the key, searching, signing in and downloading.

use super::*;

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    data: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    attributes: Attributes,
}

#[derive(Deserialize)]
struct Attributes {
    language: Option<String>,
    #[serde(default)]
    download_count: i64,
    #[serde(default)]
    hearing_impaired: bool,
    #[serde(default)]
    moviehash_match: bool,
    release: Option<String>,
    #[serde(default)]
    files: Vec<FileItem>,
}

#[derive(Deserialize)]
struct FileItem {
    file_id: i64,
}

#[derive(Deserialize)]
struct Download {
    link: Option<String>,
    reset_time_utc: Option<String>,
}

/// "2026-10-29T00:00:00.000Z" as Unix seconds.
fn unix_time(text: &str) -> Option<i64> {
    let (date, time) = text.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let min: i64 = t.next()?.parse().ok()?;
    let sec: f64 = t.next().unwrap_or("0").parse().ok()?;
    // Days since 1970 by the civil calendar (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + min * 60 + sec as i64)
}

/// Reads an answer's body, refusing one over [`MAX_ANSWER`].
async fn body(mut answer: reqwest::Response) -> Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    while let Some(chunk) = answer
        .chunk()
        .await
        .map_err(|e| Failure::Other(format!("its answer was cut off ({})", e.without_url())))?
    {
        if bytes.len() + chunk.len() > MAX_ANSWER {
            return Err(Failure::Other("its answer was too big".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn no_answer(e: reqwest::Error) -> Failure {
    Failure::Other(format!("no answer ({})", e.without_url()))
}

impl Client {
    /// A client for OpenSubtitles' API, with timeouts and no redirects (the key is in a header).
    pub fn new(settings: &OpenSubtitles) -> anyhow::Result<Self> {
        Self::with_api(settings, API)
    }

    /// A client for an API elsewhere: a stand-in, in tests.
    pub fn with_api(settings: &OpenSubtitles, api: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            settings: settings.clone(),
            api: api.trim_end_matches('/').to_owned(),
        })
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{path}", self.api))
            .header("Api-Key", self.settings.key.expose())
            .header("Accept", "application/json")
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}{path}", self.api))
            .header("Api-Key", self.settings.key.expose())
            .header("Accept", "application/json")
    }

    /// Whether OpenSubtitles takes the key and, when one is set, the login. Nothing is
    /// searched or downloaded.
    pub async fn check(&self) -> Result<(), Failure> {
        let answer = self.get("/infos/formats").send().await.map_err(no_answer)?;
        match answer.status().as_u16() {
            200 => {}
            401 | 403 => return Err(Failure::Refused),
            other => return Err(Failure::Other(format!("it answered {other}"))),
        }
        self.token(true).await.map(|_| ())
    }

    /// The account's login token, signing in when none is kept (or `afresh`); None without an
    /// account, as downloads then go under the key alone.
    async fn token(&self, afresh: bool) -> Result<Option<Secret>, Failure> {
        let Some((username, password)) = &self.settings.login else {
            return Ok(None);
        };
        if !afresh {
            let kept = LOGIN.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((who, token, at)) = kept.as_ref()
                && who == username
                && at.elapsed() < LOGIN_KEPT
            {
                return Ok(Some(token.clone()));
            }
        }
        let answer = self
            .post("/login")
            .json(&json!({ "username": username, "password": password.expose() }))
            .send()
            .await
            .map_err(no_answer)?;
        match answer.status().as_u16() {
            200 => {}
            400 | 401 | 403 => return Err(Failure::Refused),
            other => return Err(Failure::Other(format!("signing in answered {other}"))),
        }
        let bytes = body(answer).await?;
        let token = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| v["token"].as_str().map(Secret::new))
            .ok_or_else(|| Failure::Other("signing in gave no token".into()))?;
        *LOGIN.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((username.clone(), token.clone(), Instant::now()));
        Ok(Some(token))
    }

    /// The subtitles OpenSubtitles has for a video in one language, made for this file first,
    /// then the most downloaded. Searching uses no downloads.
    pub async fn search(&self, wanted: &Wanted) -> Result<Vec<Found>, Failure> {
        // OpenSubtitles redirects requests whose parameters aren't sorted and in lower case.
        let mut query: Vec<(&str, String)> = vec![("languages", wanted.language.clone())];
        if let Some(hash) = &wanted.hash {
            query.push(("moviehash", hash.clone()));
        }
        let (tmdb, imdb) = match wanted.episode {
            Some(_) => ("parent_tmdb_id", "parent_imdb_id"),
            None => ("tmdb_id", "imdb_id"),
        };
        if let Some(id) = wanted.tmdb_id {
            query.push((tmdb, id.to_string()));
        } else if let Some(id) = &wanted.imdb_id {
            query.push((imdb, id.trim_start_matches("tt").to_owned()));
        }
        if let Some((season, episode)) = wanted.episode {
            query.push(("episode_number", episode.to_string()));
            query.push(("season_number", season.to_string()));
        }
        query.sort();
        let answer = self
            .get("/subtitles")
            .query(&query)
            .send()
            .await
            .map_err(no_answer)?;
        match answer.status().as_u16() {
            200 => {}
            401 | 403 => return Err(Failure::Refused),
            other => return Err(Failure::Other(format!("searching answered {other}"))),
        }
        let search: Search = serde_json::from_slice(&body(answer).await?)
            .map_err(|_| Failure::Other("its search answer wasn't understood".into()))?;
        let mut found: Vec<Found> = search
            .data
            .into_iter()
            .filter_map(|item| {
                let a = item.attributes;
                Some(Found {
                    id: a.files.first()?.file_id,
                    release: a.release,
                    language: a.language.unwrap_or_else(|| wanted.language.clone()),
                    hearing_impaired: a.hearing_impaired,
                    downloads: a.download_count,
                    matches_file: a.moviehash_match,
                })
            })
            .collect();
        found.sort_by_key(|f| (!f.matches_file, -f.downloads));
        found.truncate(20);
        Ok(found)
    }

    /// Downloads one subtitle file, which counts against the day's downloads.
    pub async fn download(&self, file_id: i64) -> Result<Vec<u8>, Failure> {
        let mut request = self.post("/download").json(&json!({ "file_id": file_id }));
        if let Some(token) = self.token(false).await? {
            request = request.bearer_auth(token.expose());
        }
        let answer = request.send().await.map_err(no_answer)?;
        let status = answer.status().as_u16();
        let bytes = body(answer).await?;
        let download: Option<Download> = serde_json::from_slice(&bytes).ok();
        let reset = || {
            let at = download
                .as_ref()
                .and_then(|d| unix_time(d.reset_time_utc.as_deref()?));
            at.unwrap_or_else(|| crate::db::unix_now() + 24 * 3600)
        };
        let link = match status {
            200 => download.as_ref().and_then(|d| d.link.clone()),
            401 | 403 => return Err(Failure::Refused),
            406 | 429 => return Err(Failure::Quota(reset())),
            other => return Err(Failure::Other(format!("downloading answered {other}"))),
        };
        let link = link.ok_or_else(|| Failure::Other("it gave no download link".into()))?;
        self.fetch_link(&link).await
    }

    /// The file at a download link, which needs no key: it's sent nothing but the request.
    async fn fetch_link(&self, link: &str) -> Result<Vec<u8>, Failure> {
        if !link.starts_with("https://") && !link.starts_with(&self.api) {
            return Err(Failure::Other("its download link wasn't https".into()));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Failure::Other(e.without_url().to_string()))?;
        let answer = http.get(link).send().await.map_err(no_answer)?;
        if !answer.status().is_success() {
            return Err(Failure::Other(format!(
                "its download link answered {}",
                answer.status().as_u16()
            )));
        }
        body(answer).await
    }
}

/// For tests of what is sent: none of this reaches a real service.
#[cfg(test)]
pub(super) fn parsed_time(text: &str) -> Option<i64> {
    unix_time(text)
}
