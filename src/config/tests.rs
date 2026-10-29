use super::*;

#[test]
fn defaults_to_port_8100_on_all_interfaces() {
    let config = Config::from_lookup(|_| None).unwrap();
    assert_eq!(config.bind, "0.0.0.0:8100".parse::<SocketAddr>().unwrap());
}

#[test]
fn reads_bind_address() {
    let config =
        Config::from_lookup(|key| (key == "MELETE_BIND").then(|| "127.0.0.1:9000".into())).unwrap();
    assert_eq!(config.bind, "127.0.0.1:9000".parse::<SocketAddr>().unwrap());
}

#[test]
fn rejects_invalid_bind_address() {
    let err = Config::from_lookup(|_| Some("8100".into())).unwrap_err();
    assert!(err.to_string().contains("MELETE_BIND"));
}

#[test]
fn tmdb_token_is_optional_and_never_printed() {
    assert!(Config::from_lookup(|_| None).unwrap().tmdb_token.is_none());
    assert!(
        Config::from_lookup(|k| (k == "MELETE_TMDB_TOKEN").then(String::new))
            .unwrap()
            .tmdb_token
            .is_none()
    );
    let config =
        Config::from_lookup(|k| (k == "MELETE_TMDB_TOKEN").then(|| "s3cret-token".into())).unwrap();
    assert_eq!(config.tmdb_token.as_ref().unwrap().expose(), "s3cret-token");
    assert!(!format!("{config:?}").contains("s3cret"));
}

#[test]
fn transcoding_settings_default_to_the_cpu_and_are_checked() {
    let config = Config::from_lookup(|_| None).unwrap();
    assert_eq!(config.hwaccel, Hardware::Software);
    assert_eq!(config.max_transcodes, 4);
    let nvenc = Config::from_lookup(|k| (k == "MELETE_HWACCEL").then(|| "nvenc".into())).unwrap();
    assert_eq!(nvenc.hwaccel, Hardware::Nvenc);
    for (key, value) in [("MELETE_HWACCEL", "cuda"), ("MELETE_MAX_TRANSCODES", "0")] {
        let err = Config::from_lookup(|k| (k == key).then(|| value.into())).unwrap_err();
        assert!(err.to_string().contains(key), "{err}");
    }
}

#[test]
fn data_dir_defaults_to_config() {
    assert_eq!(
        Config::from_lookup(|_| None).unwrap().data_dir,
        PathBuf::from("/config")
    );
}

fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn reads_libraries_sorted_and_ignores_other_vars() {
    let libraries = parse_libraries(vars(&[
        ("MELETE_LIBRARY_SHOWS", "shows:/media/shows"),
        ("MELETE_LIBRARY_ANIME", "shows:/media/anime"),
        ("MELETE_LIBRARY_MOVIES", "movies:/media/movies"),
        ("MELETE_BIND", "0.0.0.0:8100"),
        ("PATH", "/usr/bin"),
    ]))
    .unwrap();
    let summary: Vec<_> = libraries
        .iter()
        .map(|l| (l.name.as_str(), l.kind, l.path.to_str().unwrap()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("anime", LibraryKind::Shows, "/media/anime"),
            ("movies", LibraryKind::Movies, "/media/movies"),
            ("shows", LibraryKind::Shows, "/media/shows"),
        ]
    );
}

#[test]
fn rejects_bad_library_definitions() {
    for (value, expected) in [
        ("/media/movies", "should look like"),
        ("films:/media/movies", "unknown library kind"),
        ("movies:media/movies", "must be absolute"),
    ] {
        let err = parse_libraries(vars(&[("MELETE_LIBRARY_X", value)])).unwrap_err();
        assert!(err.to_string().contains(expected), "{value}: {err}");
    }
    let err = parse_libraries(vars(&[("MELETE_LIBRARY_", "movies:/m")])).unwrap_err();
    assert!(err.to_string().contains("library names"));
}

#[test]
fn reads_library_categories() {
    let libraries = parse_library_vars(vars(&[
        ("MELETE_LIBRARY_SHOWS", "shows:/media/shows"),
        ("MELETE_LIBRARY_ANIME", "shows,anime:/media/anime"),
        ("MELETE_LIBRARY_KIDS", "movies,kids:/media/kids"),
    ]))
    .unwrap();
    let summary: Vec<_> = libraries
        .iter()
        .map(|(l, category)| (l.name.as_str(), l.kind, l.path.to_str().unwrap(), *category))
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "anime",
                LibraryKind::Shows,
                "/media/anime",
                Some(LibraryCategory::Anime)
            ),
            (
                "kids",
                LibraryKind::Movies,
                "/media/kids",
                Some(LibraryCategory::Kids)
            ),
            ("shows", LibraryKind::Shows, "/media/shows", None),
        ]
    );
    for (value, expected) in [
        ("shows,cartoons:/media/x", "unknown library category"),
        ("shows,:/media/x", "unknown library category"),
        ("music,kids:/media/x", "only movies and shows"),
        ("films,anime:/media/x", "unknown library kind"),
    ] {
        let err = parse_library_vars(vars(&[("MELETE_LIBRARY_X", value)])).unwrap_err();
        assert!(err.to_string().contains(expected), "{value}: {err}");
    }
}

#[test]
fn downloads_are_off_unless_fully_set_up() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    assert!(config(&[]).unwrap().downloads.is_none());
    assert!(!config(&[]).unwrap().musicbrainz);
    assert!(config(&[("MELETE_MUSICBRAINZ", "on")]).unwrap().musicbrainz);
    assert!(config(&[("MELETE_MUSICBRAINZ", "yes")]).is_err());
    assert!(
        config(&[
            ("MELETE_DOWNLOADS", "off"),
            ("MELETE_DELUGE_URL", "http://d")
        ])
        .unwrap()
        .downloads
        .is_none()
    );
    let on = config(&[
        ("MELETE_DOWNLOADS", "on"),
        ("MELETE_DELUGE_URL", "https://seedbox/deluge"),
        ("MELETE_DELUGE_PASSWORD", "hunter2"),
    ])
    .unwrap();
    let downloads = on.downloads.as_ref().unwrap();
    assert_eq!(downloads.deluge_url, "https://seedbox/deluge");
    assert!(
        !format!("{on:?}").contains("hunter2"),
        "the password stays out of Debug"
    );
    let err = config(&[
        ("MELETE_DOWNLOADS", "on"),
        ("MELETE_DELUGE_URL", "https://seedbox/deluge"),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("MELETE_DELUGE_PASSWORD"), "{err}");
    assert!(config(&[("MELETE_DOWNLOADS", "yes")]).is_err());
}

#[test]
fn slskd_is_off_unless_its_address_and_key_are_given() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    assert!(config(&[]).unwrap().slskd.is_none());
    assert!(config(&[("MELETE_SLSKD_URL", "")]).unwrap().slskd.is_none());
    let slskd = config(&[
        ("MELETE_SLSKD_URL", "http://slskd:5030/"),
        ("MELETE_SLSKD_API_KEY", "k3y"),
    ])
    .unwrap()
    .slskd
    .unwrap();
    assert_eq!(slskd.url, "http://slskd:5030");
    assert_eq!(slskd.api_key.expose(), "k3y");
    let err = config(&[("MELETE_SLSKD_URL", "http://slskd:5030")]).unwrap_err();
    assert!(err.to_string().contains("MELETE_SLSKD_API_KEY"), "{err}");
    assert!(
        config(&[
            ("MELETE_SLSKD_URL", "slskd:5030"),
            ("MELETE_SLSKD_API_KEY", "k")
        ])
        .is_err()
    );
    assert!(!format!("{slskd:?}").contains("k3y"), "the key never shows");
}

#[test]
fn slskds_downloads_folder_is_optional_and_absolute() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    let slskd = |pairs| config(pairs).map(|c| c.slskd.unwrap().downloads);
    assert_eq!(
        slskd(&[
            ("MELETE_SLSKD_URL", "http://slskd:5030"),
            ("MELETE_SLSKD_API_KEY", "k3y"),
        ])
        .unwrap(),
        None
    );
    assert_eq!(
        slskd(&[
            ("MELETE_SLSKD_URL", "http://slskd:5030"),
            ("MELETE_SLSKD_API_KEY", "k3y"),
            ("MELETE_SLSKD_DOWNLOADS", "/slskd-downloads"),
        ])
        .unwrap(),
        Some(PathBuf::from("/slskd-downloads"))
    );
    assert!(
        slskd(&[
            ("MELETE_SLSKD_URL", "http://slskd:5030"),
            ("MELETE_SLSKD_API_KEY", "k3y"),
            ("MELETE_SLSKD_DOWNLOADS", "slskd"),
        ])
        .is_err()
    );
}

#[test]
fn lyrics_are_off_unless_lrclib_is_chosen() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    assert!(!config(&[]).unwrap().lyrics);
    assert!(!config(&[("MELETE_LYRICS", "off")]).unwrap().lyrics);
    assert!(config(&[("MELETE_LYRICS", "lrclib")]).unwrap().lyrics);
    assert!(config(&[("MELETE_LYRICS", "on")]).is_err());
}

#[test]
fn romanisation_is_off_unless_turned_on() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    assert!(!config(&[]).unwrap().romanisation);
    assert!(
        config(&[("MELETE_LYRICS_ROMANISATION", "on")])
            .unwrap()
            .romanisation
    );
    assert!(config(&[("MELETE_LYRICS_ROMANISATION", "yes")]).is_err());
}

#[test]
fn translation_is_off_unless_libretranslate_is_set() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    assert!(config(&[]).unwrap().translation.is_none());
    assert!(
        config(&[("MELETE_LIBRETRANSLATE_API_KEY", "k")])
            .unwrap()
            .translation
            .is_none()
    );
    let set = config(&[
        ("MELETE_LIBRETRANSLATE_URL", "http://libretranslate:5000/"),
        ("MELETE_LIBRETRANSLATE_API_KEY", "k"),
    ])
    .unwrap()
    .translation
    .unwrap();
    assert_eq!(set.url, "http://libretranslate:5000");
    assert_eq!(set.api_key.unwrap().expose(), "k");
    assert!(config(&[("MELETE_LIBRETRANSLATE_URL", "ftp://x")]).is_err());
}

#[test]
fn the_stem_folders_go_together() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    assert!(config(&[]).unwrap().stems.is_none());
    let both = config(&[
        ("MELETE_STEMS_DIR", "/stems"),
        ("MELETE_STEMS_JOBS", "/stems-jobs"),
    ])
    .unwrap();
    assert_eq!(both.stems.unwrap().jobs, PathBuf::from("/stems-jobs"));
    assert!(config(&[("MELETE_STEMS_DIR", "/stems")]).is_err());
}

#[test]
fn motion_covers_are_off_until_turned_on_and_need_the_job_folder() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    let none = config(&[]).unwrap();
    assert!(!none.motion_covers);
    assert!(none.motion_jobs.is_none());
    let set = config(&[
        ("MELETE_MOTION_COVERS", "on"),
        ("MELETE_MOTION_JOBS", "/motion-jobs"),
    ])
    .unwrap();
    assert!(set.motion_covers);
    assert_eq!(set.motion_jobs, Some(PathBuf::from("/motion-jobs")));
    assert!(config(&[("MELETE_MOTION_COVERS", "yes")]).is_err());
}

#[test]
fn a_songs_album_follows_it_only_when_turned_on() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    assert!(!config(&[]).unwrap().song_albums);
    assert!(
        !config(&[("MELETE_SONG_ALBUMS", "off")])
            .unwrap()
            .song_albums
    );
    assert!(config(&[("MELETE_SONG_ALBUMS", "on")]).unwrap().song_albums);
    assert!(config(&[("MELETE_SONG_ALBUMS", "yes")]).is_err());
}

#[test]
fn the_seedbox_needs_everything_once_its_host_is_given() {
    let base = [
        ("MELETE_DOWNLOADS", "on"),
        ("MELETE_DELUGE_URL", "https://seedbox/deluge"),
        ("MELETE_DELUGE_PASSWORD", "hunter2"),
    ];
    let full = [
        ("MELETE_SEEDBOX_HOST", "seedbox.example"),
        ("MELETE_SEEDBOX_PORT", "2222"),
        ("MELETE_SEEDBOX_USER", "user"),
        ("MELETE_SEEDBOX_PASSWORD", "sftp-secret"),
        (
            "MELETE_SEEDBOX_HOST_KEY",
            "SHA256:ETvwtJPv9bESgME1L+3u5etg0pKZMA3sTgO5Dlhx2CI\r",
        ),
        ("MELETE_SEEDBOX_PATHS", "/home/user/Completed/=/Completed"),
    ];
    let config = |without: &str, replace: Option<(&str, &str)>| {
        let pairs: Vec<(String, String)> = base
            .iter()
            .chain(full.iter())
            .filter(|(k, _)| *k != without)
            .map(|(k, v)| match replace {
                Some((key, value)) if key == *k => (k.to_string(), value.to_string()),
                _ => (k.to_string(), v.to_string()),
            })
            .collect();
        Config::from_lookup(move |k| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        })
    };
    let on = config("", None).unwrap();
    let seedbox = on.downloads.as_ref().unwrap().seedbox.as_ref().unwrap();
    assert_eq!(
        (
            seedbox.port,
            seedbox.deluge_dir.as_str(),
            seedbox.sftp_dir.as_str()
        ),
        (2222, "/home/user/Completed", "/Completed")
    );
    assert!(
        !format!("{on:?}").contains("sftp-secret"),
        "the password stays out of Debug"
    );
    for key in [
        "MELETE_SEEDBOX_USER",
        "MELETE_SEEDBOX_PASSWORD",
        "MELETE_SEEDBOX_HOST_KEY",
        "MELETE_SEEDBOX_PATHS",
    ] {
        let err = config(key, None).unwrap_err();
        assert!(err.to_string().contains(key), "{err}");
    }
    assert_eq!(
        seedbox.host_key, "SHA256:ETvwtJPv9bESgME1L+3u5etg0pKZMA3sTgO5Dlhx2CI",
        "trimmed"
    );
    assert!(config("", Some(("MELETE_SEEDBOX_HOST_KEY", "abc"))).is_err());
    assert!(config("", Some(("MELETE_SEEDBOX_HOST_KEY", "SHA256:tooshort"))).is_err());
    assert!(config("", Some(("MELETE_SEEDBOX_PATHS", "/=/Completed"))).is_err());
    assert!(config("", Some(("MELETE_SEEDBOX_PATHS", "Completed=/Completed"))).is_err());
    assert!(config("", Some(("MELETE_SEEDBOX_PORT", "99999"))).is_err());
    // The rest without a host is a mistake, not "no seedbox".
    let err = config("MELETE_SEEDBOX_HOST", None).unwrap_err();
    assert!(
        err.to_string().contains("MELETE_SEEDBOX_HOST is needed"),
        "{err}"
    );
}

#[test]
fn oidc_sign_in_needs_all_its_settings_and_https() {
    let full = [
        ("MELETE_OIDC_ISSUER", "https://auth.example/realm"),
        ("MELETE_OIDC_CLIENT_ID", "melete"),
        ("MELETE_OIDC_CLIENT_SECRET", "client-secret"),
        (
            "MELETE_OIDC_REDIRECT_URL",
            "https://melete.example/api/v1/auth/oidc/callback",
        ),
    ];
    let config = |without: &str, replace: Option<(&str, &str)>| {
        let pairs: Vec<(String, String)> = full
            .iter()
            .filter(|(k, _)| *k != without)
            .map(|(k, v)| match replace {
                Some((key, value)) if key == *k => (k.to_string(), value.to_string()),
                _ => (k.to_string(), v.to_string()),
            })
            .collect();
        Config::from_lookup(move |k| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        })
    };
    assert!(Config::from_lookup(|_| None).unwrap().oidc.is_none());
    let oidc = config("", None).unwrap().oidc.unwrap();
    assert_eq!(oidc.name, "auth.example", "the issuer's host by default");
    assert_eq!(format!("{oidc:?}").matches("client-secret").count(), 0);
    for missing in [
        "MELETE_OIDC_CLIENT_ID",
        "MELETE_OIDC_CLIENT_SECRET",
        "MELETE_OIDC_REDIRECT_URL",
    ] {
        let err = config(missing, None).unwrap_err().to_string();
        assert!(err.contains(missing), "{err}");
    }
    let err = config("MELETE_OIDC_ISSUER", None).unwrap_err().to_string();
    assert!(err.contains("MELETE_OIDC_ISSUER is needed"), "{err}");
    for issuer in [
        "http://auth.example/realm",
        "https://auth.example/?x=1",
        "auth",
    ] {
        assert!(
            config("", Some(("MELETE_OIDC_ISSUER", issuer))).is_err(),
            "{issuer}"
        );
    }
    assert!(
        config(
            "",
            Some(("MELETE_OIDC_ISSUER", "http://127.0.0.1:9000/realm"))
        )
        .is_ok(),
        "plain http only on this machine"
    );
    let err = config(
        "",
        Some((
            "MELETE_OIDC_REDIRECT_URL",
            "https://melete.example/callback",
        )),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("/api/v1/auth/oidc/callback"), "{err}");
}

#[test]
fn openrouter_is_off_without_a_key_and_its_model_is_checked() {
    let config = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(|k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
    };
    assert!(config(&[]).unwrap().openrouter.is_none());
    let on = config(&[("MELETE_OPENROUTER_KEY", "sk-or-1")])
        .unwrap()
        .openrouter
        .unwrap();
    assert_eq!(on.model, DEFAULT_OPENROUTER_MODEL);
    assert!(
        !format!("{on:?}").contains("sk-or-1"),
        "the key isn't logged"
    );
    let chosen = config(&[
        ("MELETE_OPENROUTER_KEY", "sk-or-1"),
        ("MELETE_OPENROUTER_MODEL", "vendor/model-1.5:free"),
    ])
    .unwrap();
    assert_eq!(chosen.openrouter.unwrap().model, "vendor/model-1.5:free");
    assert!(config(&[("MELETE_OPENROUTER_MODEL", "vendor/model")]).is_err());
    assert!(
        config(&[
            ("MELETE_OPENROUTER_KEY", "sk-or-1"),
            ("MELETE_OPENROUTER_MODEL", "two words")
        ])
        .is_err()
    );
}

#[test]
fn apps_return_only_where_the_admin_listed() {
    let config = |returns: &'static str| {
        Config::from_lookup(move |k| match k {
            "MELETE_OIDC_ISSUER" => Some("https://auth.example/realm".into()),
            "MELETE_OIDC_CLIENT_ID" => Some("melete".into()),
            "MELETE_OIDC_CLIENT_SECRET" => Some("client-secret".into()),
            "MELETE_OIDC_REDIRECT_URL" => {
                Some("https://melete.example/api/v1/auth/oidc/callback".into())
            }
            "MELETE_OIDC_APP_RETURNS" => Some(returns.into()),
            _ => None,
        })
    };
    assert!(config("").unwrap().oidc.unwrap().app_returns.is_empty());
    let listed = config("com.example.app:/signed-in, aoide://signed-in")
        .unwrap()
        .oidc
        .unwrap();
    assert_eq!(
        listed.app_returns,
        ["com.example.app:/signed-in", "aoide://signed-in"]
    );
    for bad in ["javascript:alert(1)", "aoide://x#frag", "not a url"] {
        assert!(config(bad).is_err(), "{bad}");
    }
}

#[test]
fn languages_for_new_imports_are_none_unless_listed_and_each_is_checked() {
    let imports = |value: &'static str| {
        Config::from_lookup(move |k| match k {
            "MELETE_OPENSUBTITLES_KEY" => Some("k".into()),
            "MELETE_OPENSUBTITLES_IMPORTS" => Some(value.into()),
            _ => None,
        })
        .map(|c| c.opensubtitles.unwrap().imports)
    };
    assert!(
        imports("").unwrap().is_empty(),
        "only on request by default"
    );
    assert_eq!(imports(" EN, fr pt-BR,en ").unwrap(), ["en", "fr", "pt-br"]);
    for bad in ["english", "e", "pt-brazil", "en;fr"] {
        let err = imports(bad).unwrap_err().to_string();
        assert!(err.contains("MELETE_OPENSUBTITLES_IMPORTS"), "{bad}: {err}");
    }
}

#[test]
fn its_off_without_a_key_and_a_login_needs_both_parts() {
    let with = |pairs: &'static [(&'static str, &'static str)]| {
        Config::from_lookup(move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).into())
        })
    };
    assert!(with(&[]).unwrap().opensubtitles.is_none(), "off by default");
    let keyed = with(&[("MELETE_OPENSUBTITLES_KEY", "k")]).unwrap();
    assert!(keyed.opensubtitles.unwrap().login.is_none());
    let logged_in = with(&[
        ("MELETE_OPENSUBTITLES_KEY", "k"),
        ("MELETE_OPENSUBTITLES_USERNAME", "alice"),
        ("MELETE_OPENSUBTITLES_PASSWORD", "p"),
    ])
    .unwrap();
    assert_eq!(logged_in.opensubtitles.unwrap().login.unwrap().0, "alice");
    let half = with(&[
        ("MELETE_OPENSUBTITLES_KEY", "k"),
        ("MELETE_OPENSUBTITLES_USERNAME", "alice"),
    ]);
    assert!(half.unwrap_err().to_string().contains("go together"));
    let keyless = with(&[
        ("MELETE_OPENSUBTITLES_USERNAME", "alice"),
        ("MELETE_OPENSUBTITLES_PASSWORD", "p"),
    ]);
    assert!(
        keyless
            .unwrap_err()
            .to_string()
            .contains("MELETE_OPENSUBTITLES_KEY is needed")
    );
}

#[test]
fn metadata_language_is_english_unless_set_and_checked() {
    let language = |value: &str| {
        let value = value.to_owned();
        Config::from_lookup(move |k| (k == "MELETE_METADATA_LANGUAGE").then(|| value.clone()))
            .map(|c| c.metadata_language)
    };
    assert_eq!(
        Config::from_lookup(|_| None).unwrap().metadata_language,
        "en-US"
    );
    assert_eq!(language("").unwrap(), "en-US");
    assert_eq!(language("fr-FR").unwrap(), "fr-FR");
    for bad in ["french", "fr_FR", "fr-FR&page=2"] {
        assert!(language(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_ratings_country_is_one_melete_reads() {
    let with = |value: &'static str| {
        Config::from_lookup(move |k| (k == "MELETE_RATINGS_COUNTRY").then(|| value.to_owned()))
    };
    for fine in ["", "US", "GB", "DE"] {
        assert!(with(fine).is_ok(), "{fine}");
    }
    for bad in ["XX", "gb", "United Kingdom"] {
        assert!(with(bad).is_err(), "{bad}");
    }
}
