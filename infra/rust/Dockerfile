# syntax=docker/dockerfile:1
# Binaries and static assets are root-owned and read-only to the service user;
# only /data/storage is writable by it.
# Multi-stage install image for the Rust FVOCI slice: server, migrate, collab-engine,
# document-extract and built Vue static assets. Bun is used only by web-build;
# the runtime contains Rust helpers and no JavaScript runtime.

# These pinned images supply toolchain executables only. Every build and the
# final runtime execute on the same Ubuntu 26.04 multiarchitecture base.
FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS rust-toolchain
FROM oven/bun:1.4.2@sha256:9114c058aeae42162ee16dd5084b95fe9473970bb6bcb5b232ab1630f0546895 AS bun-toolchain
FROM ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7 AS ubuntu

FROM ubuntu AS rust-sources
COPY --from=rust-toolchain /usr/local/cargo /usr/local/cargo
COPY --from=rust-toolchain /usr/local/rustup /usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup PATH=/usr/local/cargo/bin:$PATH
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential pkg-config git curl ca-certificates \
 && rustc -vV | grep -Fx 'release: 1.98.1' \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
# Independent dev dispatcher. Own lock; not a root workspace member.
COPY xtask ./xtask
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
# Same-version libsql typed Hrana accessor; retained upstream source and local provenance.
COPY vendor/libsql-0.9.30 ./vendor/libsql-0.9.30

FROM rust-sources AS rust-deps
RUN cargo fetch --locked \
 && cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml \
 && cd crates/document-extract && bash fetch-rhwp.sh && cargo fetch --locked

FROM rust-deps AS rust-build
# Native bindgen/C prerequisites exist only in the builder (both GNU arches).
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 gcc binutils curl ca-certificates libclang-18-dev=1:18.1.8-20ubuntu8 locales \
 && localedef -i en_US -f UTF-8 en_US.UTF-8 \
 && locale -a > /tmp/fvoci-locales \
 && grep -Fx en_US.utf8 /tmp/fvoci-locales \
 && rm /tmp/fvoci-locales \
 && mkdir /sqlite-build \
 && dpkg-query -W > /sqlite-build/build-packages.txt \
 && rm -rf /var/lib/apt/lists/*
ENV LIBCLANG_PATH=/usr/lib/llvm-18/lib
# Release builds pass the tagged commit so `fvoci-server --version` can report it.
ARG FVOCI_BUILD_SHA=
ENV CARGO_TARGET_DIR=/src/target FVOCI_BUILD_SHA=${FVOCI_BUILD_SHA} CARGO_BUILD_JOBS=2
RUN bash scripts/prepare-sqlite-ci.sh --parent /sqlite-build -- cargo build --locked --release --bin fvoci-server --bin fvoci-migrate \
 && cargo build --locked --release --manifest-path crates/collab-engine/Cargo.toml --features worker --bin collab-engine \
 && cargo build --locked --release --manifest-path crates/document-extract/Cargo.toml --bin document-extract

FROM ubuntu AS web-build
COPY --from=bun-toolchain /usr/local/bin/bun /usr/local/bin/bun
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && ln -s bun /usr/local/bin/bunx \
 && rm -rf /var/lib/apt/lists/*
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

FROM ubuntu AS runtime
# No setuid/setgid files (su, passwd, mount, ...): nothing in the image
# changes user through one, and the server's uid must not reach root by one.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl postgresql-client locales \
 && localedef -i en_US -f UTF-8 en_US.UTF-8 \
 && locale -a > /tmp/fvoci-locales \
 && grep -Fx en_US.utf8 /tmp/fvoci-locales \
 && rm /tmp/fvoci-locales \
 && rm -rf /var/lib/apt/lists/* \
 && userdel --remove ubuntu \
 && if getent group ubuntu >/dev/null; then groupdel ubuntu; fi \
 && groupadd --gid 1000 fvoci \
 && useradd --uid 1000 --gid fvoci --no-create-home --home-dir /nonexistent fvoci \
 && find / -xdev -perm /6000 -type f -exec chmod ug-s {} + \
 && test -z "$(find / -xdev -perm /6000 -type f)"
WORKDIR /opt/fvoci
COPY --from=rust-build --chown=root:root /src/target/release/fvoci-server /src/target/release/fvoci-migrate ./bin/
COPY --from=rust-build --chown=root:root /src/target/release/collab-engine ./bin/
COPY --from=rust-build --chown=root:root /src/crates/vendor/yrs-0.28.0/LICENSE ./share/licenses/yrs-0.28.0/
COPY --from=rust-build --chown=root:root /src/vendor/libsql-0.9.30/LICENSE.md ./share/licenses/libsql-0.9.30/
# Original maintained timezone-data license; the compiled asset is in the server.
COPY --from=rust-build --chown=root:root /src/src/db/task_scalar_pg18_profile.LICENSE ./share/licenses/task-scalar-pg18/
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
