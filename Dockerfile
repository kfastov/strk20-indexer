# syntax=docker/dockerfile:1.7
#
# strk20 indexer — multi-stage build.
#
# Runtime is debian-slim, NOT distroless or scratch: `libsqlite3-sys` is built
# `bundled` and `zstd-sys` compiles vendored C, so the binary links glibc. A
# static musl build would have to rebuild both C libraries against musl; that
# is a real option later, but it is not free and it is not what ships today.
#
# The runtime stage also needs `ca-certificates`: reqwest here resolves TLS
# roots through `rustls-native-certs`, i.e. the OS trust store. Without the
# package every RPC call fails with an unknown-issuer error.

# ---------------------------------------------------------------- builder
# Pinned, not `rust:slim`: the toolchain is part of the build's identity.
# 1.95 is the version the workspace is developed and tested against; bookworm
# so the builder's glibc matches the runtime stage below.
FROM rust:1.95-slim-bookworm AS builder

# git + ca-certificates: `discovery-core` and the starknet-rust crates are git
# dependencies, fetched at build time.
# build-essential + pkg-config: the C compiler for the vendored SQLite / zstd.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

# `--locked` is deliberate: Cargo.lock pins the upstream engine to a rev, and
# the tag it came from is mutable upstream. A build that is allowed to
# re-resolve is a build that can silently change engines.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --release --locked -p strk20-indexerd --bin strk20 \
    && cp /build/target/release/strk20 /usr/local/bin/strk20

# ---------------------------------------------------------------- runtime
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged. A fresh named volume inherits the ownership of the image
# directory it shadows, so chowning /data here is what makes the volume
# writable by this user without any entrypoint chown dance.
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin strk20 \
    && mkdir -p /data/feed \
    && chown -R strk20:strk20 /data

COPY --from=builder /usr/local/bin/strk20 /usr/local/bin/strk20

# One volume holds both halves of an instance's state: the SQLite database
# (the working index, rebuildable) and the feed directory (the published
# product). They are kept together because an epoch file is only meaningful
# next to the database rows whose hash chain names it.
ENV STRK20_DB=/data/strk20.db \
    STRK20_FEED_DIR=/data/feed
VOLUME ["/data"]

USER strk20
WORKDIR /data
EXPOSE 8080

# `--listen 0.0.0.0:8080` lives HERE, in the container's arguments, not in the
# binary's default. Binding all interfaces is safe because a container's
# network namespace is already a boundary; the in-code default stays
# 127.0.0.1 so that running the binary on a host machine never accidentally
# publishes an unproxied indexer to the LAN.
ENTRYPOINT ["/usr/local/bin/strk20"]
CMD ["run", "--listen", "0.0.0.0:8080"]

# `/health` is 503 UNHEALTHY until the first ingest cycle finishes and writes
# `head_number`, which on a cold mainnet start is the whole backfill. The
# start period here is a placeholder; compose overrides it per network with a
# value that matches that network's backfill.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5m --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/health || exit 1
