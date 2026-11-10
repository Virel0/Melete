//! The proxy's settings: where it is, its login, and a switch for each service it may carry.

use super::*;
use crate::proxy::Service as Via;

const ON: Input = Input::Toggle { on: "on" };

/// What a switch for a service usually on the local network or inside compose warns of.
const LOCAL: &str = "Usually on the local network or inside compose, which a proxy outside can't reach: switched on, it stops working, and its API key travels through the proxy. Only for one that sits elsewhere, reached over the internet.";

pub(super) const PROXY: Service = Service {
    key: "proxy",
    name: "Proxy",
    about: "Sending the services chosen below through a proxy, such as a VPN provider's SOCKS5 or HTTP proxy, so they see its address rather than the server's. Each goes direct until it's switched on here. One switched on only ever goes through the proxy: while the proxy is down or unset its requests fail, they never go direct, and System's status says so. Test asks api.ipify.org through the proxy which address the outside world sees: that's an outside request, made only when Test is pressed. The password stays on the server. Each indexer has a switch of its own, under Indexers.",
    fields: &[
        field(
            "MELETE_PROXY_TYPE",
            "Type",
            "SOCKS5 has the proxy look names up too, so they aren't looked up from the server.",
            Input::Choice {
                options: &[
                    Choice::new("socks5", "SOCKS5"),
                    Choice::new("http", "HTTP"),
                    Choice::new("https", "HTTPS (TLS to the proxy)"),
                ],
            },
        ),
        field(
            "MELETE_PROXY_HOST",
            "Host",
            "The proxy's name or IP address alone, e.g. 192.168.1.10 or a provider's proxy host.",
            Input::Text,
        ),
        field(
            "MELETE_PROXY_PORT",
            "Port",
            "e.g. 1080 for SOCKS5.",
            Input::Number,
        ),
        field(
            "MELETE_PROXY_USERNAME",
            "Username",
            "Only for a proxy that wants a login.",
            Input::Text,
        ),
        field("MELETE_PROXY_PASSWORD", "Password", "", Input::Secret),
        field(
            Via::Tmdb.setting(),
            "TMDB through the proxy",
            "Its API and its image server.",
            ON,
        ),
        field(
            Via::MusicBrainz.setting(),
            "MusicBrainz through the proxy",
            "",
            ON,
        ),
        field(
            Via::CoverArt.setting(),
            "Cover Art Archive through the proxy",
            "Album covers, from coverartarchive.org and archive.org.",
            ON,
        ),
        field(
            Via::Wikimedia.setting(),
            "Wikimedia through the proxy",
            "Artists' portraits, from wikidata.org and Wikimedia Commons.",
            ON,
        ),
        field(
            Via::ListenBrainz.setting(),
            "ListenBrainz through the proxy",
            "For You, from labs.api.listenbrainz.org and api.listenbrainz.org.",
            ON,
        ),
        field(Via::Lyrics.setting(), "LRCLIB through the proxy", "", ON),
        field(
            Via::OpenSubtitles.setting(),
            "OpenSubtitles through the proxy",
            "",
            ON,
        ),
        field(
            Via::OpenRouter.setting(),
            "OpenRouter through the proxy",
            "",
            ON,
        ),
        field(
            Via::Lists.setting(),
            "IMDb and Rotten Tomatoes through the proxy",
            "Their charts, for import lists.",
            ON,
        ),
        field(
            Via::Translation.setting(),
            "LibreTranslate through the proxy",
            "At the address set for it; one in the household can't be reached through a proxy outside.",
            ON,
        ),
        field(
            Via::Oidc.setting(),
            "The identity provider (OIDC) through the proxy",
            "Melete's own requests to it; browsers still go to it directly. One in the household can't be reached through a proxy outside.",
            ON,
        ),
        field(
            Via::Deluge.setting(),
            "Deluge through the proxy",
            "Its web UI on the seedbox.",
            ON,
        ),
        field(
            Via::Seedbox.setting(),
            "The seedbox's SFTP through the proxy",
            "Where finished downloads are fetched from, and the host key the setup command reads. Through an HTTP or HTTPS proxy that lets connections out to the seedbox's SSH port (CONNECT), or SOCKS5.",
            ON,
        ),
        field(
            Via::Arr.setting(),
            "Sonarr, Radarr, Lidarr and Prowlarr through the proxy",
            LOCAL,
            ON,
        ),
        field(Via::Slskd.setting(), "slskd through the proxy", LOCAL, ON),
        field(
            Via::FlareSolverr.setting(),
            "FlareSolverr through the proxy",
            LOCAL,
            ON,
        ),
    ],
};
