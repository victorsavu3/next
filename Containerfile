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

COPY --from=builder /app/target/release/next-mcp /usr/local/bin/

EXPOSE 3000
VOLUME /data/tasks

CMD ["next-mcp"]
