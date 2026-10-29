//! Reading the outside services' settings (FlareSolverr, slskd, LibreTranslate, downloading,
//! OpenRouter, the identity provider and the seedbox), refusing incomplete or unsafe ones.

use super::*;

/// FlareSolverr's address (`MELETE_FLARESOLVERR_URL`): off when empty, http or https otherwise.
/// `MELETE_SCAN_SCHEDULE`, checked: read where it's used, as it can change while running.
pub(super) fn check_scan_schedule(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<()> {
    if let Some(value) = get("MELETE_SCAN_SCHEDULE") {
        crate::jobs::scans::Schedule::parse(&value).map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// `MELETE_RATINGS_COUNTRY`, checked: one whose ratings Melete reads. Read where it's used, as
/// it can change while running.
pub(super) fn check_ratings_country(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<()> {
    match get(crate::ratings::COUNTRY_SETTING).filter(|v| !v.trim().is_empty()) {
        Some(value) if crate::ratings::known_country(value.trim()).is_none() => bail!(
            "MELETE_RATINGS_COUNTRY={value:?} should be one of the countries whose ratings Melete reads, such as US or GB"
        ),
        _ => Ok(()),
    }
}

/// `MELETE_METADATA_LANGUAGE`: a TMDB language such as `fr-FR`, `en-US` when unset or empty.
pub(super) fn metadata_language(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<String> {
    match get("MELETE_METADATA_LANGUAGE").filter(|v| !v.is_empty()) {
        None => Ok(crate::tmdb::language::ENGLISH.into()),
        Some(language) if crate::tmdb::language::is_language(&language) => Ok(language),
        Some(other) => bail!(
            "MELETE_METADATA_LANGUAGE={other:?} should be a language and region such as fr-FR"
        ),
    }
}

pub(super) fn flaresolverr(
    get: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<String>> {
    Ok(
        match get("MELETE_FLARESOLVERR_URL").filter(|u| !u.is_empty()) {
            Some(url) => {
                let parsed = reqwest::Url::parse(&url)
                    .with_context(|| format!("MELETE_FLARESOLVERR_URL {url:?} isn't a URL"))?;
                anyhow::ensure!(
                    matches!(parsed.scheme(), "http" | "https"),
                    "MELETE_FLARESOLVERR_URL is http:// or https://"
                );
                Some(url.trim_end_matches('/').to_owned())
            }
            None => None,
        },
    )
}

/// slskd's settings: off without an address; with one, its API key is needed.
pub(super) fn slskd(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Slskd>> {
    Ok(match get("MELETE_SLSKD_URL").filter(|u| !u.is_empty()) {
        Some(url) => {
            let parsed = reqwest::Url::parse(&url)
                .with_context(|| format!("MELETE_SLSKD_URL {url:?} isn't a URL"))?;
            anyhow::ensure!(
                matches!(parsed.scheme(), "http" | "https"),
                "MELETE_SLSKD_URL is http:// or https://"
            );
            let api_key = get("MELETE_SLSKD_API_KEY")
                .filter(|k| !k.is_empty())
                .context("MELETE_SLSKD_URL is set, so MELETE_SLSKD_API_KEY is needed")?;
            let downloads = match get("MELETE_SLSKD_DOWNLOADS").filter(|d| !d.is_empty()) {
                Some(dir) => {
                    let dir = PathBuf::from(dir);
                    anyhow::ensure!(dir.is_absolute(), "MELETE_SLSKD_DOWNLOADS must be absolute");
                    Some(dir)
                }
                None => None,
            };
            Some(Slskd {
                url: url.trim_end_matches('/').to_owned(),
                api_key: Secret(api_key),
                downloads,
            })
        }
        None => None,
    })
}

/// LibreTranslate's settings: off without an address, its API key optional.
pub(super) fn translation(
    get: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<Translation>> {
    Ok(
        match get("MELETE_LIBRETRANSLATE_URL").filter(|u| !u.is_empty()) {
            Some(url) => {
                let parsed = reqwest::Url::parse(&url)
                    .with_context(|| format!("MELETE_LIBRETRANSLATE_URL {url:?} isn't a URL"))?;
                anyhow::ensure!(
                    matches!(parsed.scheme(), "http" | "https"),
                    "MELETE_LIBRETRANSLATE_URL is http:// or https://"
                );
                Some(Translation {
                    url: url.trim_end_matches('/').to_owned(),
                    api_key: get("MELETE_LIBRETRANSLATE_API_KEY")
                        .filter(|k| !k.is_empty())
                        .map(Secret),
                })
            }
            None => None,
        },
    )
}

/// Downloading's settings: off unless asked for; asked for, it needs everything or nothing
/// starts.
pub(super) fn downloads(
    get: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<Downloads>> {
    Ok(match get("MELETE_DOWNLOADS").as_deref() {
        None | Some("off") => None,
        Some("on") => {
            let required = |key: &str| {
                get(key)
                    .filter(|v| !v.is_empty())
                    .with_context(|| format!("MELETE_DOWNLOADS=on needs {key}"))
            };
            Some(Downloads {
                deluge_url: required("MELETE_DELUGE_URL")?,
                deluge_password: Secret(required("MELETE_DELUGE_PASSWORD")?),
                seedbox: match get("MELETE_SEEDBOX_HOST").filter(|h| !h.is_empty()) {
                    Some(host) => Some(seedbox(host, get)?),
                    None => {
                        no_seedbox_settings(get)?;
                        None
                    }
                },
            })
        }
        Some(other) => anyhow::bail!("MELETE_DOWNLOADS={other:?} should be on or off"),
    })
}

/// Without a seedbox host, refuses any other seedbox setting.
fn no_seedbox_settings(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<()> {
    // All or nothing both ways: a mistyped host shouldn't silently
    // leave the rest unused.
    let others = ["USER", "PASSWORD", "HOST_KEY", "PATHS"];
    if let Some(set) = others
        .iter()
        .map(|o| format!("MELETE_SEEDBOX_{o}"))
        .find(|key| get(key).is_some_and(|v| !v.is_empty()))
    {
        anyhow::bail!("{set} is set, so MELETE_SEEDBOX_HOST is needed too");
    }
    Ok(())
}

/// Where Melete may talk to an identity provider: https, or plain http on this machine only.
/// Melete trusts the ID token the provider hands it directly because TLS says who handed it
/// (OpenID Connect Core 3.1.3.7), so anything else would let the network vouch for anyone.
pub fn oidc_url_allowed(url: &reqwest::Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }),
        _ => false,
    }
}

/// OpenRouter's settings: off without a key; a model without one is a mistake.
pub(super) fn openrouter(
    get: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<OpenRouter>> {
    let value = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let model = value("MELETE_OPENROUTER_MODEL");
    let Some(key) = value("MELETE_OPENROUTER_KEY") else {
        if model.is_some() {
            bail!("MELETE_OPENROUTER_MODEL is set, so MELETE_OPENROUTER_KEY is needed too");
        }
        return Ok(None);
    };
    let model = model.unwrap_or_else(|| DEFAULT_OPENROUTER_MODEL.to_owned());
    anyhow::ensure!(
        model.len() <= 100
            && model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._:-".contains(&b)),
        "MELETE_OPENROUTER_MODEL is an OpenRouter model id, like {DEFAULT_OPENROUTER_MODEL}"
    );
    Ok(Some(OpenRouter {
        key: Secret(key),
        model,
    }))
}

/// OpenSubtitles' settings: off without a key; a login needs both its parts, and a key.
pub(super) fn opensubtitles(
    get: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<OpenSubtitles>> {
    let value = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let key = value("MELETE_OPENSUBTITLES_KEY");
    let username = value("MELETE_OPENSUBTITLES_USERNAME");
    // A password may start or end with a space.
    let password = get("MELETE_OPENSUBTITLES_PASSWORD").filter(|v| !v.is_empty());
    let login = match (username, password) {
        (Some(username), Some(password)) => Some((username, Secret(password))),
        (None, None) => None,
        _ => bail!(
            "MELETE_OPENSUBTITLES_USERNAME and MELETE_OPENSUBTITLES_PASSWORD go together: set both, or neither"
        ),
    };
    let Some(key) = key else {
        if login.is_some() {
            bail!("an OpenSubtitles login is set, so MELETE_OPENSUBTITLES_KEY is needed too");
        }
        return Ok(None);
    };
    let mut imports: Vec<String> = Vec::new();
    for code in get("MELETE_OPENSUBTITLES_IMPORTS")
        .unwrap_or_default()
        .split([',', ' '])
        .filter(|c| !c.trim().is_empty())
    {
        let Some(language) = crate::opensubtitles::language_code(code) else {
            bail!(
                "MELETE_OPENSUBTITLES_IMPORTS lists languages as two letters, or two and a region, separated by commas: en, fr, pt-br ({code:?} isn't one)"
            );
        };
        if !imports.contains(&language) {
            imports.push(language);
        }
    }
    Ok(Some(OpenSubtitles {
        key: Secret(key),
        login,
        imports,
    }))
}

/// OIDC sign-in's settings: none, or all of them that are needed.
pub(super) fn oidc(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<Oidc>> {
    let value = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let Some(issuer) = value("MELETE_OIDC_ISSUER") else {
        if let Some(set) = ["CLIENT_ID", "CLIENT_SECRET", "REDIRECT_URL"]
            .iter()
            .map(|k| format!("MELETE_OIDC_{k}"))
            .find(|k| value(k).is_some())
        {
            bail!("{set} is set, so MELETE_OIDC_ISSUER is needed too");
        }
        return Ok(None);
    };
    let parsed = reqwest::Url::parse(&issuer)
        .with_context(|| format!("MELETE_OIDC_ISSUER {issuer:?} isn't a URL"))?;
    anyhow::ensure!(
        oidc_url_allowed(&parsed) && parsed.query().is_none() && parsed.fragment().is_none(),
        "MELETE_OIDC_ISSUER must be an https:// address without ? or #"
    );
    let required = |key: &str| {
        value(key).with_context(|| format!("MELETE_OIDC_ISSUER is set, so {key} is needed too"))
    };
    let client_id = required("MELETE_OIDC_CLIENT_ID")?;
    let client_secret = Secret(required("MELETE_OIDC_CLIENT_SECRET")?);
    let redirect_url = required("MELETE_OIDC_REDIRECT_URL")?;
    let redirect = reqwest::Url::parse(&redirect_url)
        .with_context(|| format!("MELETE_OIDC_REDIRECT_URL {redirect_url:?} isn't a URL"))?;
    anyhow::ensure!(
        matches!(redirect.scheme(), "http" | "https")
            && redirect.path().ends_with("/api/v1/auth/oidc/callback")
            && redirect.query().is_none()
            && redirect.fragment().is_none(),
        "MELETE_OIDC_REDIRECT_URL is Melete's address followed by /api/v1/auth/oidc/callback"
    );
    let name = value("MELETE_OIDC_NAME").unwrap_or_else(|| {
        parsed
            .host_str()
            .unwrap_or("your account provider")
            .to_owned()
    });
    anyhow::ensure!(
        name.chars().count() <= 60,
        "MELETE_OIDC_NAME is at most 60 characters"
    );
    let app_returns = value("MELETE_OIDC_APP_RETURNS")
        .unwrap_or_default()
        .split([' ', ','])
        .filter(|r| !r.is_empty())
        .map(|r| {
            let url = reqwest::Url::parse(r)
                .ok()
                .filter(|u| !NEVER_RETURN.contains(&u.scheme()) && u.fragment().is_none())
                .with_context(|| {
                    format!("MELETE_OIDC_APP_RETURNS: {r:?} isn't an app's address to return to, like com.example.app:/signed-in")
                })?;
            anyhow::ensure!(r.len() <= 500, "MELETE_OIDC_APP_RETURNS: {r:?} is too long");
            Ok(String::from(url))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Some(Oidc {
        issuer,
        client_id,
        client_secret,
        redirect_url,
        name,
        app_returns,
    }))
}

/// The seedbox's SFTP settings: all of them, once the host is given.
pub(super) fn seedbox(
    host: String,
    get: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Seedbox> {
    let required = |key: &str| {
        get(key)
            .filter(|v| !v.is_empty())
            .with_context(|| format!("MELETE_SEEDBOX_HOST is set, so {key} is needed too"))
    };
    // Trimmed: a Windows-edited .env leaves a \r that would refuse every connection.
    let host_key = required("MELETE_SEEDBOX_HOST_KEY")?.trim().to_owned();
    let valid = host_key.strip_prefix("SHA256:").is_some_and(|b64| {
        b64.len() == 43
            && b64
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    });
    if !valid {
        anyhow::bail!(
            "MELETE_SEEDBOX_HOST_KEY should be SHA256: and 43 characters, as OpenSSH writes it (melete seedbox host-key <host> <port> shows it)"
        );
    }
    let paths = required("MELETE_SEEDBOX_PATHS")?;
    let (deluge_dir, sftp_dir) = paths
        .split_once('=')
        .filter(|(d, s)| d.starts_with('/') && !d.trim_end_matches('/').is_empty() && s.starts_with('/'))
        .with_context(|| {
            format!("MELETE_SEEDBOX_PATHS={paths:?} should be <deluge folder>=<sftp folder>, both absolute (and Deluge's not /)")
        })?;
    let port = match get("MELETE_SEEDBOX_PORT") {
        Some(value) => value
            .parse()
            .with_context(|| format!("MELETE_SEEDBOX_PORT={value:?} isn't a port"))?,
        None => 22,
    };
    let trim = |p: &str| match p.trim_end_matches('/') {
        "" => "/".to_owned(),
        trimmed => trimmed.to_owned(),
    };
    Ok(Seedbox {
        host,
        port,
        user: required("MELETE_SEEDBOX_USER")?,
        password: Secret(required("MELETE_SEEDBOX_PASSWORD")?),
        host_key,
        deluge_dir: trim(deluge_dir),
        sftp_dir: trim(sftp_dir),
    })
}
