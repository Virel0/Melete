# Melete's trailer downloader: fetches a title's trailer from YouTube and converts it once so
# every player can play it, taking jobs as files Melete writes. Python because yt-dlp is; Melete
# itself stays in Rust. The same base and the same ffmpeg (jellyfin-ffmpeg, with NVENC) as
# Melete's own image; yt-dlp and the JavaScript runtime it needs for YouTube pinned exactly.
FROM debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
# The build stops unless the key file holds exactly one key, the Jellyfin Team's.
ARG JELLYFIN_KEY_FINGERPRINT=4918AABC486CA052358D778D49023CD01DE21A7B
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gnupg python3 python3-venv \
    && curl -fsSL https://repo.jellyfin.org/jellyfin_team.gpg.key -o /tmp/jellyfin.asc \
    && gpg --show-keys --with-colons /tmp/jellyfin.asc \
        | awk -F: '/^pub/ { keys++ } /^fpr/ && !fpr { fpr = $10 } END { exit !(keys == 1 && fpr == ENVIRON["JELLYFIN_KEY_FINGERPRINT"]) }' \
    && gpg --dearmor -o /usr/share/keyrings/jellyfin.gpg /tmp/jellyfin.asc \
    && echo "deb [signed-by=/usr/share/keyrings/jellyfin.gpg] https://repo.jellyfin.org/debian trixie main" \
        > /etc/apt/sources.list.d/jellyfin.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends jellyfin-ffmpeg8=8.1.3-1-trixie \
    && apt-get purge -y curl gnupg \
    && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/* /tmp/jellyfin.asc /root/.gnupg
COPY requirements.txt /app/requirements.txt
RUN python3 -m venv /app/venv \
    && /app/venv/bin/pip install --no-cache-dir -r /app/requirements.txt
COPY fetch.py /app/fetch.py

# Melete's own user, so it can read and move what's written.
RUN useradd --uid 10001 --no-create-home --shell /usr/sbin/nologin melete
USER 10001
ENV HOME=/tmp PYTHONUNBUFFERED=1 PATH=/app/venv/bin:/usr/lib/jellyfin-ffmpeg:/usr/local/bin:/usr/bin:/bin
ENTRYPOINT ["python3", "/app/fetch.py"]
