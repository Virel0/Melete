//! The VPN beside Melete (Gluetun, from compose.vpn.yaml): its control server, as Melete reaches
//! it.

use super::*;

pub(super) const VPN: Service = Service {
    key: "vpn",
    name: "VPN (Gluetun)",
    about: "The tunnel compose.vpn.yaml runs in Gluetun, which Soulseek, the trailer downloader and the services sent through its proxy share. Here Melete shows how it is (running or not, the address the outside world sees and its country, the forwarded port), and starts, stops and restarts it. Which provider and servers it uses (SERVER_COUNTRIES and the rest) is set in Gluetun's own settings file, which Melete never reads or changes: its key can't send the tunnel anywhere else. To send services through it, set Proxy to HTTP at host gluetun, port 8888, with the proxy login .env gives Gluetun. Melete reaches Gluetun's control server inside compose, with a key of its own; nothing leaves the server for this. A control server that answers without a key is refused, as anyone on its network could stop the VPN.",
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
    ],
};
