# syntax=docker/dockerfile:1
FROM rust:1.99-slim-trixie@sha256:1a939748a7cbab172de91a014e3804a97af1394a8c844faa67fce9401c4e40fb AS build
WORKDIR /src

# Build dependencies first so source edits don't invalidate this layer.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

# Migrations are compiled into the binary.
COPY migrations ./migrations
COPY src ./src
RUN touch src/main.rs && cargo build --release --locked

FROM debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
# CA certificates for HTTPS to TMDB, a font for burned-in subtitles, and jellyfin-ffmpeg (ffmpeg with NVENC/NVDEC and CUDA
# tone mapping) from Jellyfin's repository. The build stops unless the key file holds exactly
# one key, the Jellyfin Team's.
ARG JELLYFIN_KEY_FINGERPRINT=4918AABC486CA052358D778D49023CD01DE21A7B
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
    && curl -fsSL https://repo.jellyfin.org/jellyfin_team.gpg.key -o /tmp/jellyfin.asc \
    && gpg --show-keys --with-colons /tmp/jellyfin.asc \
        | awk -F: '/^pub/ { keys++ } /^fpr/ && !fpr { fpr = $10 } END { exit !(keys == 1 && fpr == ENVIRON["JELLYFIN_KEY_FINGERPRINT"]) }' \
    && gpg --dearmor -o /usr/share/keyrings/jellyfin.gpg /tmp/jellyfin.asc \
    && echo "deb [signed-by=/usr/share/keyrings/jellyfin.gpg] https://repo.jellyfin.org/debian trixie main" \
        > /etc/apt/sources.list.d/jellyfin.list \
    && apt-get update \
        && apt-get install -y --no-install-recommends jellyfin-ffmpeg8=8.1.3-1-trixie fonts-dejavu-core \
    && apt-get purge -y curl gnupg \
    && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/* /tmp/jellyfin.asc /root/.gnupg \
    && groupadd --system --gid 10001 melete \
    && useradd --system --uid 10001 --gid melete --no-create-home melete \
    && install -d -o melete -g melete /config /transcode
COPY --from=build /src/target/release/melete /usr/local/bin/melete
# The root filesystem is read-only; fontconfig (for burned-in subtitles) caches here instead.
ENV XDG_CACHE_HOME=/transcode/.cache
USER melete
EXPOSE 8100
CMD ["/usr/local/bin/melete"]
