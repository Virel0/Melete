//! What a Test leaves out: a service's stored secrets and usernames, when the values tried send
//! it somewhere other than where it's set up. Otherwise an admin's Test of a new address would
//! hand the stored password, key or login to whatever answers there.

use super::*;

/// Whether `name` says where a service is reached: its address, host, port, kind of proxy or
/// issuer. Melete's own callback address (`…_REDIRECT_URL`) isn't one: nothing is sent there.
fn address(name: &str) -> bool {
    ["_HOST", "_PORT", "_TYPE", "_ISSUER"]
        .iter()
        .any(|end| name.ends_with(end))
        || (name.ends_with("_URL") && !name.ends_with("_REDIRECT_URL"))
}

/// Whether `field` is a login: a secret, or the user it goes with.
fn login(field: &Field) -> bool {
    field.input == Input::Secret
        || field.name.ends_with("_USERNAME")
        || field.name.ends_with("_USER")
}

/// Clears from `changes`, for a Test, each of service `key`'s logins the body didn't give, when
/// it changes where the service is reached (as `env`, else `stored`, has it); returns what was
/// left out, by label. Refused, saying why, when one left out is set in `.env`, which a Test can't
/// leave out.
pub fn withhold(
    key: &str,
    changes: &mut HashMap<String, Option<String>>,
    stored: &HashMap<String, String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<&'static str>, String> {
    let Some(service) = SERVICES.iter().find(|s| s.key == key) else {
        return Ok(Vec::new());
    };
    let now = |name: &str| {
        env(name)
            .filter(|v| !v.trim().is_empty())
            .or_else(|| stored.get(name).cloned())
    };
    let moved = changes.iter().any(|(name, value)| {
        let value = value.as_deref().map(str::trim).filter(|v| !v.is_empty());
        address(name) && value != now(name).as_deref().map(str::trim)
    });
    if !moved {
        return Ok(Vec::new());
    }
    let mut withheld = Vec::new();
    for field in service.fields.iter().filter(|f| login(f)) {
        if changes.contains_key(field.name) || now(field.name).is_none() {
            continue;
        }
        if env(field.name).is_some_and(|v| !v.trim().is_empty()) {
            return Err(format!(
                "{} is set in .env, so it isn't tried at an address other than the one set up. Save the new address first, then test.",
                field.label
            ));
        }
        changes.insert(field.name.to_owned(), None);
        withheld.push(field.label);
    }
    Ok(withheld)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored() -> HashMap<String, String> {
        [
            ("MELETE_PROXY_HOST", "192.168.1.10"),
            ("MELETE_PROXY_PORT", "1080"),
            ("MELETE_PROXY_USERNAME", "alice"),
            ("MELETE_PROXY_PASSWORD", "pw"),
            ("MELETE_DELUGE_URL", "https://seedbox.example/deluge"),
            ("MELETE_DELUGE_PASSWORD", "pw"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    fn changes(pairs: &[(&str, &str)]) -> HashMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Some(v.to_string())))
            .collect()
    }

    const NO_ENV: &dyn Fn(&str) -> Option<String> = &|_| None;

    #[test]
    fn a_new_address_leaves_the_stored_login_out() {
        let mut moved = changes(&[("MELETE_PROXY_HOST", "203.0.113.9")]);
        let left = withhold("proxy", &mut moved, &stored(), NO_ENV).unwrap();
        assert_eq!(left, ["Username", "Password"]);
        assert_eq!(moved["MELETE_PROXY_PASSWORD"], None);
        assert_eq!(moved["MELETE_PROXY_USERNAME"], None);

        let mut deluge = changes(&[("MELETE_DELUGE_URL", "https://elsewhere.example/")]);
        let left = withhold("downloads", &mut deluge, &stored(), NO_ENV).unwrap();
        assert_eq!(left, ["Web UI password"]);
    }

    #[test]
    fn the_same_address_or_a_login_given_keeps_it() {
        let mut same = changes(&[
            ("MELETE_PROXY_HOST", "192.168.1.10"),
            ("MELETE_PROXY_TMDB", "on"),
        ]);
        assert!(
            withhold("proxy", &mut same, &stored(), NO_ENV)
                .unwrap()
                .is_empty()
        );
        assert!(!same.contains_key("MELETE_PROXY_PASSWORD"));

        let mut given = changes(&[
            ("MELETE_PROXY_PORT", "1081"),
            ("MELETE_PROXY_USERNAME", "bob"),
            ("MELETE_PROXY_PASSWORD", "other"),
        ]);
        assert!(
            withhold("proxy", &mut given, &stored(), NO_ENV)
                .unwrap()
                .is_empty()
        );
        assert_eq!(given["MELETE_PROXY_PASSWORD"].as_deref(), Some("other"));
    }

    #[test]
    fn one_set_in_env_refuses_the_test() {
        let env = |name: &str| (name == "MELETE_PROXY_PASSWORD").then(|| "from-env".to_owned());
        let mut moved = changes(&[("MELETE_PROXY_HOST", "203.0.113.9")]);
        let refused = withhold("proxy", &mut moved, &stored(), &env).unwrap_err();
        assert!(refused.starts_with("Password is set in .env"), "{refused}");
        assert!(!refused.contains("from-env"));
    }
}
