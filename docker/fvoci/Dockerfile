# syntax=docker/dockerfile:1
#
# The fvoci server with its web app, in one image. The build context is the repository
# root; what it may read is in Dockerfile.dockerignore.
#
#   docker build -f docker/fvoci/Dockerfile -t fvoci-server \
#       --build-arg FVOCI_BUILD_HASH=$(git rev-parse HEAD) \
#       --build-arg FVOCI_BUILD_DATE=$(git log -1 --format=%cI) .
#   (without the two, ping's commit and buildDate are empty)
#   docker run --init -p 127.0.0.1:8080:8000 -e FVOCI_SERVER_URL=http://localhost:8080 \
#       -v fvoci-data:/data fvoci-server
#
# Alone it serves SQLite under /data. Settings are FVOCI_* variables or a file mounted
# over /opt/fvoci/config.json; --init lets a stop end a server that is still starting.
# With the argument --migrate it brings a database's schema up to date as the role of
# FVOCI_DATABASE_CONNECTION, the owner of the tables, and exits. docker/compose.yaml runs it with
# PostgreSQL and the search server; `cargo xtask docker` builds both images, starts
# that and tries it, and `cargo xtask docker --push-plan` prints how they go to GHCR.
#
# Nothing here depends on the processor: the same file builds linux/arm64 and
# linux/amd64, each natively on a machine of that kind. TLS is rustls throughout, so
# the image needs no OpenSSL library. The health check is the server asking itself
# (`--check-ready`), since the image has no HTTP client. The worker that holds the
# documents edited together is this executable too, started by the server.
#
# Secrets are never build arguments or layers. At run time docker/fvoci/entrypoint.sh
# reads the files of Docker secrets into the variables the server reads.
#
# Metrics are off. To serve them, set FVOCI_METRICS_ADDRESS=0.0.0.0:9092, publish that
# port to the monitoring network only, and give FVOCI_METRICS_TOKEN_FILE (a file of at
# least 32 characters, such as `openssl rand -hex 32`): the listener is plain HTTP.

ARG RUST_VERSION=1
ARG NODE_VERSION=24

# The web app: pnpm as package.json's packageManager pins it (corepack checks its
# hash), the dependencies exactly as the lockfile has them. The fonts it builds are
# downloads pinned by their SHA-256 (apps/web/tools/fonts/sources.json).
FROM node:${NODE_VERSION}-trixie-slim AS web
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable
WORKDIR /src
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json apps/web/
COPY vendor/rhwp-studio/package.json vendor/rhwp-studio/
RUN --mount=type=cache,id=fvoci-pnpm-store,target=/root/.cache/pnpm-store \
    pnpm install --frozen-lockfile --store-dir /root/.cache/pnpm-store
COPY apps/web apps/web
COPY vendor vendor
# The licences the bundle may ship under.
COPY deny.toml ./
RUN --mount=type=cache,id=fvoci-web-fonts,target=/src/target/web-fonts \
    pnpm --filter web run build

# Debian 13 for the server and the image: a prebuilt part of the TLS library needs its
# C library (glibc 2.38 or newer); with Debian 12 the link step fails.
FROM rust:${RUST_VERSION}-trixie AS build
# The processor the image is built for, which keeps apart the caches of the builds.
ARG TARGETARCH
# The commit the server is built from and its time, for ping (`commit`, `buildDate`)
# and the metrics' fvoci_build_info; empty when not given.
ARG FVOCI_BUILD_HASH=
ARG FVOCI_BUILD_DATE=
ENV FVOCI_BUILD_HASH=${FVOCI_BUILD_HASH} FVOCI_BUILD_DATE=${FVOCI_BUILD_DATE}
# The cache of cargo's target directory, shared by every build of the machine; the
# probe of `cargo xtask docker` builds with one of its own.
ARG FVOCI_CARGO_TARGET_CACHE=fvoci-cargo-target
WORKDIR /src
COPY Cargo.toml Cargo.lock clippy.toml ./
COPY crates ./crates
# A member of the workspace; cargo needs its manifest even though it is not built here.
COPY xtask ./xtask
# cargo takes a source for unchanged when it is older than what the cache built from it,
# and a copied file keeps the time it had in the context: a checkout or an archive older
# than the cache's last build (another commit's, another worktree's) would get that
# build's executable. Every source is dated in 2100 first, later than anything cargo
# can have written whatever the builder's clock says, so the workspace's crates are
# always compiled from what was copied; the dependencies stay cached. Docker's cache of
# this step goes by the contents it was sent, but it sends a file again only when its
# time or size changed since the same context directory was last sent: do not rewrite a
# context in place keeping its files' times.
RUN --mount=type=cache,id=fvoci-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=${FVOCI_CARGO_TARGET_CACHE}-${TARGETARCH},target=/src/target,sharing=locked \
    find Cargo.toml Cargo.lock clippy.toml crates xtask -exec touch -c -d @4102444800 {} + \
    && cargo build --release --locked -p fvoci-server \
    && cp target/release/fvoci-server /fvoci-server

FROM debian:trixie-slim
# Certificate authorities for outgoing HTTPS: webhooks, S3, mail, sign-in providers,
# the search server.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --home-dir /data --create-home fvoci \
    && mkdir -p /data/files \
    && chown -R fvoci:fvoci /data
COPY --from=build /fvoci-server /opt/fvoci/fvoci-server
COPY --from=web /src/apps/web/dist/ /opt/fvoci/web/
COPY docker/fvoci/config.json /opt/fvoci/config.json
COPY --chmod=0755 docker/fvoci/entrypoint.sh /opt/fvoci/entrypoint.sh
USER fvoci
WORKDIR /data
VOLUME /data
EXPOSE 8000
# Every 30 s once started (each check writes a small file to the storage), every 2 s
# while it starts.
HEALTHCHECK --interval=30s --timeout=5s --start-period=120s --start-interval=2s --retries=3 \
    CMD ["/opt/fvoci/fvoci-server", "--config", "/opt/fvoci/config.json", "--check-ready"]
ENTRYPOINT ["/opt/fvoci/entrypoint.sh"]

# Labels last, so that a new commit or date does not rebuild the layers above.
# `source` names the public repository the image is built from.
ARG FVOCI_BUILD_HASH=
ARG FVOCI_CREATED=
ARG FVOCI_VERSION=
LABEL org.opencontainers.image.title="fvoci-server" \
      org.opencontainers.image.description="The fvoci server and its web app" \
      org.opencontainers.image.source="https://github.com/AISFlow/fvoci" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.revision="${FVOCI_BUILD_HASH}" \
      org.opencontainers.image.created="${FVOCI_CREATED}" \
      org.opencontainers.image.version="${FVOCI_VERSION}"
