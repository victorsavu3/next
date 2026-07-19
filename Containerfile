FROM rust:1-alpine AS builder

RUN apk add --no-cache \
    musl-dev \
    openssl-dev \
    libgit2-dev \
    pkgconf \
    zlib-dev

WORKDIR /app
COPY . .
# Use dynamic linking against musl (avoids needing static versions of all deps).
ENV RUSTFLAGS="-C target-feature=-crt-static"
RUN cargo build --release --features mcp --bin next-mcp

# ── Runtime image ─────────────────────────────────────────────────────────────
FROM alpine:3

RUN apk add --no-cache \
    libgcc \
    libgit2 \
    openssl \
    ca-certificates \
    git

# Unprivileged service user. UID 1000 matches typical rootless-Podman host UID.
RUN addgroup -S -g 1000 next \
 && adduser  -S -G next -u 1000 -h /home/next -s /sbin/nologin next \
 && mkdir -p /home/next /data/tasks /data/state /data/config/next-mcp \
 && chown -R next:next /home/next /data/tasks /data/state /data/config

COPY --from=builder /app/target/release/next-mcp /usr/local/bin/

# In rootless Podman the host user's UID may not match the container UID after
# user-namespace mapping, causing git's ownership check to fail on bind mounts.
# This container is single-purpose and all repositories it touches are trusted.
RUN git config --system safe.directory '*'

# XDG_STATE_HOME / XDG_CONFIG_HOME at well-known paths inside the container so
# the quadlet volume mounts are predictable regardless of the home directory.
# XDG_CONFIG_HOME=/data/config makes next-mcp's default config path resolve to
# /data/config/next-mcp/config.toml.
ENV HOME=/home/next
ENV XDG_STATE_HOME=/data/state
ENV XDG_CONFIG_HOME=/data/config

USER next
EXPOSE 3000

# Persistent data lives under /data — operators must mount storage here.
# Do NOT declare VOLUME instructions: they cause Podman/Docker to create
# anonymous volumes for subdirectories when a parent path is mounted, which
# silently shadows the operator's mount and loses data on container restart.
# See quadlets/next-mcp.container for the recommended volume configuration.

CMD ["next-mcp"]
