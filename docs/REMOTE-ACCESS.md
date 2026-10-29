# Reaching Melete from outside

Melete doesn't open anything to the internet itself. You choose the ways in, and Melete tells
them apart by the address each connection comes from, opening only what you've listed for each
(decision 76). A common setup is two ways in, besides home:

- **WireGuard for your own devices.** A WireGuard server on the media server's host. Phones and
  laptops connect to it and reach Melete as if they were at home: video, music, everything.
- **A reverse proxy on a VPS for the rest.** Caddy or nginx on a small VPS holds a public HTTPS
  address (say `music.example.org`) and forwards to Melete through a WireGuard tunnel from the
  VPS to home. Nothing at home is open to the internet, and you choose what it carries: music
  and requests, for instance, while video stays on WireGuard.

All addresses below are examples. Pick your own ranges, as long as they don't clash with your
home network.

| What | Example |
|------|---------|
| Home network | `192.168.0.0/24` |
| WireGuard for your devices | `10.10.0.0/24`, the server at `10.10.0.1` |
| The tunnel to the VPS | `10.20.0.0/24`: the VPS at `10.20.0.1`, home at `10.20.0.2` |
| Melete | port `8100` on the media server |

## 1. WireGuard for your devices

On the media server's host (not in Melete's container), `/etc/wireguard/wg0.conf`:

```ini
[Interface]
Address = 10.10.0.1/24
ListenPort = 51820
PrivateKey = <the server's private key>

[Peer]
# A phone
PublicKey = <the phone's public key>
AllowedIPs = 10.10.0.2/32
```

Forward UDP port 51820 on your router to the media server, then `wg-quick up wg0` (and
`systemctl enable wg-quick@wg0`). On each device, the peer is the server's public key, your
home's public address and port as the endpoint, and `AllowedIPs = 10.10.0.1/32, 192.168.0.0/24`.
The apps then reach Melete at `http://192.168.0.10:8100` (the media server's home address) or
`http://10.10.0.1:8100`.

## 2. The tunnel from the VPS to home

The VPS listens and home connects out to it, so nothing at home needs opening for this.

On the VPS, `/etc/wireguard/wg0.conf`:

```ini
[Interface]
Address = 10.20.0.1/24
ListenPort = 51820
PrivateKey = <the VPS's private key>

[Peer]
# Home
PublicKey = <home's tunnel public key>
AllowedIPs = 10.20.0.2/32
```

At home, a second interface, `/etc/wireguard/wg1.conf`:

```ini
[Interface]
Address = 10.20.0.2/24
PrivateKey = <home's tunnel private key>

[Peer]
# The VPS
PublicKey = <the VPS's public key>
Endpoint = <the VPS's public address>:51820
AllowedIPs = 10.20.0.1/32
PersistentKeepalive = 25
```

The VPS then reaches Melete at `http://10.20.0.2:8100`, and Melete sees it come from `10.20.0.1`.

## 3. The reverse proxy on the VPS

Point `music.example.org` at the VPS. Either proxy gets a certificate for it and forwards to
Melete through the tunnel. Melete takes uploads of up to 32 MB (Aoide's sync), keeps live
updates open as long-lived event streams, and streams music, so the proxy must let all three
through.

### Caddy

`/etc/caddy/Caddyfile`:

```caddy
music.example.org {
	request_body {
		max_size 32MB
	}
	reverse_proxy 10.20.0.2:8100 {
		# Live updates are sent as they happen, not buffered.
		flush_interval -1
	}
}
```

Caddy gets the certificate itself and sends `X-Forwarded-For` and `X-Forwarded-Proto` on its own.

### nginx

With a certificate from certbot (`certbot --nginx -d music.example.org`), a server block:

```nginx
server {
    listen 443 ssl;
    server_name music.example.org;
    # ssl_certificate and ssl_certificate_key as certbot writes them

    client_max_body_size 32m;

    location / {
        proxy_pass http://10.20.0.2:8100;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        # Live updates and streams are sent as they happen, and may stay open a long time.
        proxy_buffering off;
        proxy_read_timeout 1h;
    }
}
```

## 4. Telling Melete

In `.env`:

```sh
MELETE_WAYS_IN=home 192.168.0.0/24 all; wireguard 10.10.0.0/24 all max=8M; vps 10.20.0.1 music,requests
MELETE_TRUSTED_PROXIES=10.20.0.1
```

- `MELETE_WAYS_IN` lists each way in: a name, its addresses or ranges (comma-separated), and what
  it opens: any of `video`, `music`, `requests` and `admin` (comma-separated), or `all`.
  Optionally, a video limit: `max=` and bits per second with `k`, `M` or `G` (`max=8M`), for a
  way whose upload is limited.
  - A file over the limit isn't sent as it is through that way (playing or downloading it);
    the apps are told the limit and ask for a conversion, which is held to it (the video gets
    the limit less 384 kbit/s for the sound). Music isn't affected.
  - The limit is the way's, whatever the app's quality setting says. `/api/v1/speedtest`
    still measures what the connection really carries.
  - **video**: browsing movies and series, playing and streaming them.
  - **music**: the music library, playing it, and everything the Aoide apps sync.
  - **requests**: asking for things, following them, importing playlists.
  - **admin**: settings, Wanted, downloads, users, approving requests.
  - Signing in, your own account and the web page itself go through every way in. With the
    example above, signing in at `music.example.org` works, music and requests work, and
    asking for a video or an admin page says it isn't open through that way.
  - Once it's set, an address in none of the ways reaches nothing. Leave it empty and every
    address reaches everything, as before.
- `MELETE_TRUSTED_PROXIES` lists the proxies whose word on who the client is Melete takes (from
  `X-Forwarded-For`). Without it, every visitor through the VPS looks like the VPS to Melete:
  a few wrong passwords from one of them would lock everyone out of that account for 15
  minutes. Only list proxies you run; anyone else could claim to be anyone.

Restart Melete (`docker compose up -d`) after changing either.

## 5. Checking it

Melete has to see each connection's real address. Docker's published ports normally keep it for
traffic arriving over WireGuard or the home network, but some setups (Docker's userland proxy
for IPv6, for instance) replace it with Docker's own, and then every connection looks alike.

To check, list only one way in, say `home 192.168.0.0/24 all`, and open Melete over WireGuard.
It should refuse you, and its log (`docker logs melete`) says which address it saw:

```
refused by the ways in peer=10.10.0.2 path=/api/v1/me why="this address isn't one of Melete's ways in"
```

If `peer` is your device's WireGuard address, it works; put the full list back. If it's a Docker
address (`172.x.x.x`), Melete can't tell the ways apart as it runs. Running it with
`network_mode: host` keeps the addresses.

## What each way shows to whom

- Over WireGuard, traffic is encrypted end to end between your device and home.
- Through the VPS, the tunnel to home is encrypted, but HTTPS ends on the VPS: whoever runs the
  VPS could see what passes through it, sign-ins included. Open through it only what you'd
  trust it with.
- Nothing here sends anything to anyone else; Melete itself is still the only thing at home
  talking to the proxy.
