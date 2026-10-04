# syntax=docker/dockerfile:1
# Binaries and static assets are root-owned and read-only to the service user;
# only /data/storage is writable by it.
# Multi-stage install image for the Rust FVOCI slice: server, migrate, collab-engine,
# document-extract and built React static assets. Bun is used only by web-build;
# the runtime contains Rust helpers and no JavaScript runtime.

FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS rust-sources
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY migrations ./migrations
COPY src ./src
# `fvoci-migrate --init-env` renders this template (include_str!).
COPY infra/rust/.env.example ./infra/rust/.env.example
# `/api/docs/json` embeds the committed OpenAPI export (include_str!).
COPY apps/web/openapi.json ./apps/web/openapi.json
COPY scripts ./scripts
COPY crates/collab-engine ./crates/collab-engine
# FVOCI-owned same-version read-only Yrs accessor patch (direct path dependency).
COPY crates/vendor/yrs-0.28.0 ./crates/vendor/yrs-0.28.0
COPY crates/document-extract ./crates/document-extract
COPY crates/document-extract-client ./crates/document-extract-client
# markdown-rs with one local fix, wired by `[patch.crates-io]` (vendor/markdown/PATCHES.md).
COPY vendor/markdown ./vendor/markdown

FROM rust-sources AS rust-deps
RUN cargo fetch --locked \
 && cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml \
 && cd crates/document-extract && bash fetch-rhwp.sh && cargo fetch --locked

FROM rust-deps AS rust-build
# Native bindgen/C prerequisites exist only in the builder (both GNU arches).
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 gcc binutils curl ca-certificates libclang-14-dev=1:14.0.6-12 \
 && mkdir /sqlite-build \
 && dpkg-query -W > /sqlite-build/build-packages.txt \
 && rm -rf /var/lib/apt/lists/*
ENV LIBCLANG_PATH=/usr/lib/llvm-14/lib
# Release builds pass the tagged commit so `fvoci-server --version` can report it.
ARG FVOCI_BUILD_SHA=
ENV CARGO_TARGET_DIR=/src/target FVOCI_BUILD_SHA=${FVOCI_BUILD_SHA}
RUN bash scripts/prepare-sqlite-ci.sh --parent /sqlite-build -- cargo build --locked --release --bin fvoci-server --bin fvoci-migrate \
 && cargo build --locked --release --manifest-path crates/collab-engine/Cargo.toml --features worker --bin collab-engine \
 && cargo build --locked --release --manifest-path crates/document-extract/Cargo.toml --bin document-extract

FROM oven/bun:1.4.2@sha256:9114c058aeae42162ee16dd5084b95fe9473970bb6bcb5b232ab1630f0546895 AS web-build
WORKDIR /src
# The Bun workspace (package.json workspaces, bun.lock, bunfig.toml's linker)
# spans apps/web, packages/* and scripts/document-convert; `bun ci` needs every
# member manifest.
COPY package.json bun.lock bunfig.toml .bun-version ./
# `patchedDependencies` in package.json: bun ci applies these to the install.
COPY patches ./patches
COPY scripts/document-convert/package.json ./scripts/document-convert/
COPY packages ./packages
COPY LICENSE ./LICENSE
COPY third-party/browser-licenses ./third-party/browser-licenses
COPY apps/web/package.json ./apps/web/
# One pinned Bun: this image's, packageManager and the .bun-version CI uses.
RUN bun -e 'const v = (await Bun.file(".bun-version").text()).trim(); \
  const pm = (await Bun.file("package.json").json()).packageManager; \
  if (Bun.version !== v || pm !== `bun@${v}`) throw new Error(`Bun ${Bun.version}, packageManager ${pm}, .bun-version ${v}`)' \
 && bun ci
# The committed openapi.json / generated api.ts are kept in sync with the Rust
# DTOs by the Web workflow's generate-api drift check, so this stage does not
# depend on the Rust build.
COPY apps/web ./apps/web/
WORKDIR /src/apps/web
RUN bun --bun run build

FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS runtime
# No setuid/setgid files (su, passwd, mount, ...): nothing in the image
# changes user through one, and the server's uid must not reach root by one.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl postgresql-client \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 1000 fvoci \
 && useradd --uid 1000 --gid fvoci --no-create-home --home-dir /nonexistent fvoci \
 && find / -xdev -perm /6000 -type f -exec chmod ug-s {} + \
 && test -z "$(find / -xdev -perm /6000 -type f)"
WORKDIR /opt/fvoci
COPY --from=rust-build --chown=root:root /src/target/release/fvoci-server /src/target/release/fvoci-migrate ./bin/
COPY --from=rust-build --chown=root:root /src/target/release/collab-engine ./bin/
COPY --from=rust-build --chown=root:root /src/crates/vendor/yrs-0.28.0/LICENSE ./share/licenses/yrs-0.28.0/
COPY --from=rust-build --chown=root:root /src/target/release/document-extract ./bin/
# PDF export fonts (SIL OFL 1.1), read by the export child at run time (FVOCI_EXPORT_FONT_DIR).
COPY --chown=root:root packages/editor/src/fonts/*.ttf ./share/fonts/
COPY --chown=root:root packages/editor/src/fonts/*-OFL.txt ./share/licenses/fonts/
# Swagger UI served at /api/docs (Apache-2.0): license, notice and bundled third-party notices.
COPY --chown=root:root src/http/api_docs_assets/LICENSE src/http/api_docs_assets/NOTICE src/http/api_docs_assets/swagger-ui-bundle.js.LICENSE.txt ./share/licenses/swagger-ui-dist-5.33.0/
COPY --from=web-build --chown=root:root /src/apps/web/dist ./static/
COPY --chown=root:root infra/rust/init-db.sh ./sbin/init-db.sh
# /run/fvoci/meili is the service user's for the env-file stack's `init`; the
# Compose install's root preparation takes it over (root:root 0755) before
# writing the key there (src/search/meili.rs `prepare_key_dir`).
RUN chmod 755 ./sbin/init-db.sh ./bin/* \
 && mkdir -p /data/storage /run/fvoci/meili \
 && chown fvoci:fvoci /data/storage /run/fvoci/meili
ENV FVOCI_EXPORT_FONT_DIR=/opt/fvoci/share/fonts \
    FVOCI_STATIC_DIR=/opt/fvoci/static \
    FVOCI_COLLAB_ENGINE=/opt/fvoci/bin/collab-engine \
    FVOCI_EXTRACTOR_BIN=/opt/fvoci/bin/document-extract \
    FVOCI_STORAGE_DIR=/data/storage \
    FVOCI_BIND=0.0.0.0:8080
# The image runs as fvoci (uid 1000) by default. The Compose install
# (compose.user.yml) starts it as root instead, so the preparation owns the
# search key directory and docker exec / healthcheck processes (which start
# with the passwords from the container configuration) are root's; it then
# runs the server as this uid/gid.
USER fvoci
EXPOSE 8080
# `fvoci-migrate --start` execs fvoci-server (same pid, arguments passed on).
# It first refuses the retired <VAR>_FILE settings of older compose files.
# Given the database owner password (the Compose install: POSTGRES_PASSWORD)
# it then validates the settings, waits for PostgreSQL and Meilisearch, creates
# the app role, migrates, grants and writes the scoped search key; the server
# starts without the owner password or master key and, if this ran as root, as
# uid/gid 1000 with no supplementary groups (src/prepare.rs).
ENTRYPOINT ["/opt/fvoci/bin/fvoci-migrate", "--start"]
