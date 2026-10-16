# CI tools and pinned native SQLite SDK; no application source or credentials.
# Borrow the Debian bookworm CA bundle from the digest-pinned Rust image only
# for HTTPS bootstrap before apt installs Ubuntu's ca-certificates package.
FROM rust:1.99.0-bookworm@sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6 AS rust-toolchain
RUN rustup set profile minimal \
 && rustup component add --toolchain 1.99.0 rustfmt clippy \
 && if rustup component list --installed | grep -q '^rust-docs'; then rustup component remove rust-docs; fi \
 && rustup set auto-self-update disable \
 && chmod -R go-w /usr/local/rustup /usr/local/cargo

# Ubuntu's index includes native linux/amd64 and linux/arm64 images.
FROM ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7 AS apt-source

ARG UBUNTU_SNAPSHOT=20261008T000000Z
ENV LIBCLANG_PATH=/usr/lib/llvm-18/lib
# Archive signatures remain mandatory; historical Release expiry is disabled
# for this fixed snapshot, including Playwright's later apt commands.
RUN printf '%s\n' \
      'Types: deb' \
      "URIs: https://snapshot.ubuntu.com/ubuntu/$UBUNTU_SNAPSHOT/" \
      'Suites: resolute resolute-updates resolute-security' \
      'Components: main restricted universe multiverse' \
      'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' \
      'Check-Valid-Until: no' \
      > /etc/apt/sources.list.d/ubuntu.sources \
 && cat /etc/apt/sources.list.d/ubuntu.sources

FROM apt-source AS system-tools
RUN --mount=type=bind,from=rust-toolchain,source=/etc/ssl/certs/ca-certificates.crt,target=/tmp/bootstrap-ca.crt,readonly \
    apt-get -o Acquire::https::CAInfo=/tmp/bootstrap-ca.crt -o APT::Update::Error-Mode=any update \
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::https::CAInfo=/tmp/bootstrap-ca.crt install -y --no-install-recommends \
      ca-certificates \
 && test -s /etc/ssl/certs/ca-certificates.crt \
 && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      curl git gcc g++ make binutils libc6-dev pkg-config \
      unzip xz-utils zstd sudo \
      libclang-18-dev=1:18.1.8-20ubuntu8 \
 && dpkg-query -W gcc binutils libc6 libc6-dev libclang-18-dev curl \
      > /usr/local/share/ci-base-build-packages.txt \
 && cat /usr/local/share/ci-base-build-packages.txt \
 && rm -rf /var/lib/apt/lists/*

# The official Ubuntu image may already reserve uid/gid 1000 for "ubuntu".
RUN if id ubuntu >/dev/null 2>&1; then userdel --remove ubuntu; fi \
 && if getent group ubuntu >/dev/null; then groupdel ubuntu; fi \
 && groupadd --gid 1000 ci \
 && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash ci

FROM system-tools AS rust-tools
COPY --from=rust-toolchain --chown=0:0 /usr/local/rustup /opt/rustup
COPY --from=rust-toolchain --chown=0:0 /usr/local/cargo /opt/cargo
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/home/ci/.cargo \
    PATH=/opt/cargo/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin
RUN install -d -o 1000 -g 1000 -m 0755 /home/ci/.cargo /home/ci/.cargo/registry /home/ci/.cargo/git \
 && test "$(rustc --version | cut -d' ' -f2)" = 1.99.0 \
 && cargo --version && cargo fmt --version && cargo clippy --version

# Download the same checksum-pinned native Bun independently of Ubuntu APT.
# Only its binary is copied; Debian's packages and CA bundle stay in this stage.
FROM rust-toolchain AS bun-tool
RUN case "$(dpkg --print-architecture)" in \
      amd64) archive=bun-linux-x64; checksum=3e730528d89775f03f87f05c06787c4df4099d0a37f3f252e7b829cb23816e18 ;; \
      arm64) archive=bun-linux-aarch64; checksum=efa9813da5ed72423bf847f916e8d2c47c0d776add972354026a75e10da9aa21 ;; \
      *) exit 1 ;; \
    esac \
 && curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
      "https://github.com/oven-sh/bun/releases/download/bun-v1.4.3/$archive.zip" -o /tmp/bun.zip \
 && printf '%s  %s\n' "$checksum" /tmp/bun.zip | sha256sum --check --strict \
 && unzip -q /tmp/bun.zip -d /tmp/bun \
 && install -m 0755 "/tmp/bun/$archive/bun" /usr/local/bin/bun \
 && ln -s bun /usr/local/bin/bunx \
 && test "$(bun --version)" = 1.4.3 \
 && rm -rf /tmp/bun /tmp/bun.zip

# Prefetch Playwright's complete native APT dependency closure while the common
# tools install. APT still owns resolution, signed indexes and archive hashes.
# No mutable cache is imported: this branch shares the exact Ubuntu/source/Bun
# build graph, and the final installer validates its own package selection.
FROM apt-source AS ci-web-debs
COPY --from=bun-tool --chown=0:0 /usr/local/bin/bun /usr/local/bin/bun
RUN --mount=type=bind,from=rust-toolchain,source=/etc/ssl/certs/ca-certificates.crt,target=/tmp/bootstrap-ca.crt,readonly \
    apt-get -o Acquire::https::CAInfo=/tmp/bootstrap-ca.crt -o APT::Update::Error-Mode=any update \
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::https::CAInfo=/tmp/bootstrap-ca.crt install -y --no-install-recommends ca-certificates \
 && test -s /etc/ssl/certs/ca-certificates.crt \
 && mkdir /tmp/ci-web /ci-web-archives \
 && cd /tmp/ci-web \
 && printf '%s\n' '{"private":true,"dependencies":{"@playwright/test":"1.64.0"}}' > package.json \
 && bun install --ignore-scripts \
 && test "$(bun --bun x --no-install playwright --version)" = 'Version 1.64.0' \
 && printf '%s\n' 'APT::Get::Download-Only "true";' 'Dir::Cache::archives "/ci-web-archives";' > /etc/apt/apt.conf.d/99-ci-web-prefetch \
 && bun --bun x --no-install playwright install-deps chromium \
 && test -n "$(find /ci-web-archives -maxdepth 1 -name '*.deb' -print -quit)"

FROM rust-tools AS ci-base-tools
# CI tools use pinned official binaries.
COPY --from=bun-tool --chown=0:0 /usr/local/bin/bun /usr/local/bin/bun
RUN ln -s bun /usr/local/bin/bunx \
 && test "$(bun --version)" = 1.4.3

RUN version=0.9.148 \
 && case "$(dpkg --print-architecture)" in \
      amd64) target=x86_64-unknown-linux-gnu; checksum=223ba936714cc861a9cdedad5a8eb664dfdd69d6fbdf6cea6aa8184f523290dc ;; \
      arm64) target=aarch64-unknown-linux-gnu; checksum=81469eb963ef883fd590e60189e77d6aba3d91b418591d3391f50fe3e9480d2b ;; \
      *) exit 1 ;; \
    esac \
 && curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
      "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$version/cargo-nextest-$version-$target.tar.gz" -o /tmp/nextest.tar.gz \
 && printf '%s  %s\n' "$checksum" /tmp/nextest.tar.gz | sha256sum --check --strict \
 && mkdir /tmp/nextest \
 && tar -xzf /tmp/nextest.tar.gz -C /tmp/nextest cargo-nextest \
 && install -o 0 -g 0 -m 0755 /tmp/nextest/cargo-nextest /usr/local/bin/cargo-nextest \
 && test "$(cargo nextest --version | sed -n '1p' | cut -d' ' -f2)" = "$version" \
 && rm -rf /tmp/nextest /tmp/nextest.tar.gz

USER 1000:1000
WORKDIR /home/ci

# Browser-only extension of the exact pinned common recipe above. Building both
# targets avoids inventing a GHCR parent digest before its first publication.
FROM ci-base-tools AS ci-web-tools
USER root
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/playwright
RUN --mount=type=bind,from=ci-web-debs,source=/ci-web-archives,target=/tmp/ci-web-prefetched,readonly \
    cp -a /tmp/ci-web-prefetched /tmp/ci-web-archives \
 && printf '%s\n' 'Dir::Cache::archives "/tmp/ci-web-archives";' > /etc/apt/apt.conf.d/99-ci-web-archives \
 && mkdir /tmp/ci-web \
 && cd /tmp/ci-web \
 && printf '%s\n' '{"private":true,"dependencies":{"@playwright/test":"1.64.0"}}' > package.json \
 && bun install --ignore-scripts \
 && test "$(bun --bun x --no-install playwright --version)" = 'Version 1.64.0' \
 && cat /etc/apt/sources.list.d/ubuntu.sources \
 && bun --bun x --no-install playwright install --with-deps chromium \
 && dpkg-query -W > /usr/local/share/ci-web-build-packages.txt \
 && cat /usr/local/share/ci-web-build-packages.txt \
 && rm /etc/apt/apt.conf.d/99-ci-web-archives \
 && rm -rf /tmp/ci-web /tmp/ci-web-archives /root/.bun /var/lib/apt/lists/*
USER 1000:1000

# Compile the existing task tool in a builder-only source stage. Both image
# variants use this exact binary; neither final image contains the checkout.
FROM ci-base-tools AS sqlite-tool-build
USER root
COPY xtask/Cargo.toml xtask/Cargo.lock /tmp/ci-sqlite-tool/xtask/
COPY xtask/src /tmp/ci-sqlite-tool/xtask/src
COPY xtask/*.tsv /tmp/ci-sqlite-tool/xtask/
COPY tests/support/official_sqld.rs /tmp/ci-sqlite-tool/tests/support/official_sqld.rs
RUN --mount=type=cache,target=/tmp/sqlite-ci-cargo \
    --mount=type=cache,target=/tmp/sqlite-ci-target \
    CARGO_HOME=/tmp/sqlite-ci-cargo CARGO_BUILD_JOBS=2 \
      cargo build --locked --release --manifest-path /tmp/ci-sqlite-tool/xtask/Cargo.toml \
      --target-dir /tmp/sqlite-ci-target --jobs 2 \
 && install -o 0 -g 0 -m 0755 /tmp/sqlite-ci-target/release/xtask /usr/local/bin/fvoci-ci-tools

# Native inputs are measured in each variant's own final compiler/header/ABI
# environment. Browser APT closure must not borrow the base SDK's identity.
FROM ci-base-tools AS sqlite-base-build
USER root
COPY --from=sqlite-tool-build /usr/local/bin/fvoci-ci-tools /usr/local/bin/fvoci-ci-tools
RUN mkdir -m 0755 /opt/fvoci-sqlite \
 && /usr/local/bin/fvoci-ci-tools sqlite-ci --seal-image-sdk --parent /opt/fvoci-sqlite \
 && chmod -R a-w /opt/fvoci-sqlite
# Mutable build prefixes start private; the sealed public SDK is readable by any UID.
RUN chmod -R a+rX,a-w /opt/fvoci-sqlite

FROM ci-web-tools AS sqlite-web-build
USER root
COPY --from=sqlite-tool-build /usr/local/bin/fvoci-ci-tools /usr/local/bin/fvoci-ci-tools
# Reuse the authenticated archive download, then build against this actual SDK.
COPY --from=sqlite-base-build /opt/fvoci-sqlite/sqlite-amalgamation-3530400.zip /opt/fvoci-sqlite/sqlite-amalgamation-3530400.zip
RUN /usr/local/bin/fvoci-ci-tools sqlite-ci --seal-image-sdk --parent /opt/fvoci-sqlite \
 && chmod -R a-w /opt/fvoci-sqlite
RUN chmod -R a+rX,a-w /opt/fvoci-sqlite

FROM ci-base-tools AS ci-base
COPY --from=sqlite-tool-build --chown=0:0 /usr/local/bin/fvoci-ci-tools /usr/local/bin/fvoci-ci-tools
COPY --from=sqlite-base-build --chown=0:0 /opt/fvoci-sqlite /opt/fvoci-sqlite
USER 1000:1000

FROM ci-web-tools AS ci-web
COPY --from=sqlite-tool-build --chown=0:0 /usr/local/bin/fvoci-ci-tools /usr/local/bin/fvoci-ci-tools
COPY --from=sqlite-web-build --chown=0:0 /opt/fvoci-sqlite /opt/fvoci-sqlite
USER 1000:1000
