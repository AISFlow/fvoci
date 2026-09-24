# syntax=docker/dockerfile:1
# Multi-stage install image for the Rust FVOCI slice: server, migrate, collab-engine,
# document-extract, and built React static assets. Runtime ships helpers enabled via env.

FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS rust-sources
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY migrations ./migrations
COPY src ./src
COPY scripts ./scripts
COPY crates/collab-engine ./crates/collab-engine
COPY crates/document-extract ./crates/document-extract
COPY crates/document-extract-client ./crates/document-extract-client

FROM rust-sources AS rust-deps
RUN cargo fetch --locked \
 && cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml \
 && cd crates/document-extract && bash fetch-rhwp.sh && cargo fetch --locked

FROM rust-deps AS rust-build
ENV CARGO_TARGET_DIR=/src/target
RUN cargo build --locked --release --bin fvoci-server --bin fvoci-migrate \
 && cargo build --locked --release --manifest-path crates/collab-engine/Cargo.toml --features worker --bin collab-engine \
 && cargo build --locked --release --manifest-path crates/document-extract/Cargo.toml --bin document-extract

FROM node:24-bookworm-slim@sha256:0e0ff40c39bc087845bfb27465a0df4ea419520094bc35842ff83dd8cbe6f9b6 AS web-build
WORKDIR /src
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY packages ./packages
COPY apps/web/package.json apps/web/package-lock.json ./apps/web/
WORKDIR /src/apps/web
RUN npm ci --install-links --no-audit --no-fund \
 && npm ci --prefix ../../packages/editor --ignore-scripts --no-audit --no-fund
# The committed openapi.json / generated api.ts are kept in sync with the Rust
# DTOs by the Web workflow's generate-api drift check, so this stage does not
# depend on the Rust build.
COPY apps/web ./
RUN npm run build

FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl postgresql-client \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 1000 fvoci \
 && useradd --uid 1000 --gid fvoci --no-create-home --home-dir /nonexistent fvoci
WORKDIR /opt/fvoci
COPY --from=rust-build --chown=fvoci:fvoci /src/target/release/fvoci-server /src/target/release/fvoci-migrate ./bin/
COPY --from=rust-build --chown=fvoci:fvoci /src/target/release/collab-engine ./bin/
COPY --from=rust-build --chown=fvoci:fvoci /src/target/release/document-extract ./bin/
COPY --from=web-build --chown=fvoci:fvoci /src/apps/web/dist ./static/
COPY --chown=root:root infra/rust/init-db.sh ./sbin/init-db.sh
RUN chmod 755 ./sbin/init-db.sh ./bin/* \
 && mkdir -p /data/storage \
 && chown fvoci:fvoci /data/storage
ENV FVOCI_STATIC_DIR=/opt/fvoci/static \
    FVOCI_COLLAB_ENGINE=/opt/fvoci/bin/collab-engine \
    FVOCI_EXTRACTOR_BIN=/opt/fvoci/bin/document-extract \
    FVOCI_STORAGE_DIR=/data/storage \
    FVOCI_BIND=0.0.0.0:8080
USER fvoci
EXPOSE 8080
ENTRYPOINT ["/opt/fvoci/bin/fvoci-server"]
