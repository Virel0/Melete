//! The VPN beside Melete (Gluetun, from compose.vpn.yaml): its control server, and the server
//! countries to keep.

use super::*;

pub(super) const VPN: Service = Service {
    key: "vpn",
    name: "VPN (Gluetun)",
    about: "The tunnel compose.vpn.yaml runs in Gluetun, which Soulseek, the trailer downloader and the services sent through its proxy share. Here Melete shows how it is (running or not, the address the outside world sees, the forwarded port), starts, stops and restarts it, and keeps the server countries chosen. To send services through it, set Proxy to HTTP at host gluetun, port 8888, with the proxy login .env gives Gluetun. Melete reaches Gluetun's control server inside compose, with a key of its own; nothing leaves the server for this. The VPN's own keys stay in Gluetun's files, never here. A control server that answers without a key is refused, as anyone on its network could stop the VPN.",
    fields: &[
        field(
            "MELETE_VPN_URL",
            "Control server",
            "As Melete reaches it, e.g. http://gluetun:8000.",
            Input::Text,
        ),
        field(
            "MELETE_VPN_KEY",
            "Melete's key",
            "The API key of the melete role in Gluetun's roles file (gluetun-roles.example.toml shows it).",
            Input::Secret,
        ),
        field(
            crate::gluetun::COUNTRIES_SETTING,
            "Server countries",
            "Optional: countries as your VPN provider's server list names them, such as Netherlands, Sweden. Applied when saved (the VPN reconnects) and again after Gluetun restarts. Empty: Gluetun's own settings choose.",
            Input::Text,
        ),
    ],
};
