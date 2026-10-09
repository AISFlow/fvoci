# Tool-only CI image; no application source, credentials or SQLite prefix.
# Borrow the Debian bookworm CA bundle from the digest-pinned Rust image only
# for HTTPS bootstrap before apt installs Ubuntu's ca-certificates package.
FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS rust-toolchain
RUN rustup set profile minimal \
 && rustup component add --toolchain 1.98.1 rustfmt clippy \
 && if rustup component list --installed | grep -q '^rust-docs'; then rustup component remove rust-docs; fi \
 && rustup set auto-self-update disable \
 && chmod -R go-w /usr/local/rustup /usr/local/cargo

# Ubuntu's index includes native linux/amd64 and linux/arm64 images.
FROM ghcr.io/aisflow/fvoci/ci/ubuntu@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7 AS system-tools

ARG UBUNTU_SNAPSHOT=20261008T000000Z
ENV LIBCLANG_PATH=/usr/lib/llvm-18/lib
# Archive signatures remain mandatory; historical Release expiry is disabled
# for this fixed snapshot, including Playwright's later apt commands.
# python3: 임시: C12a Python 감사 때 제거
# python3-yaml: 임시: C4에서 PyYAML 사용처 제거 시 삭제
RUN --mount=type=bind,from=rust-toolchain,source=/etc/ssl/certs/ca-certificates.crt,target=/tmp/bootstrap-ca.crt,readonly \
    printf '%s\n' \
      'Types: deb' \
      "URIs: https://snapshot.ubuntu.com/ubuntu/$UBUNTU_SNAPSHOT/" \
      'Suites: resolute resolute-updates resolute-security' \
      'Components: main restricted universe multiverse' \
      'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' \
      'Check-Valid-Until: no' \
      > /etc/apt/sources.list.d/ubuntu.sources \
 && cat /etc/apt/sources.list.d/ubuntu.sources \
 && apt-get -o Acquire::https::CAInfo=/tmp/bootstrap-ca.crt -o APT::Update::Error-Mode=any update \
 && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::https::CAInfo=/tmp/bootstrap-ca.crt install -y --no-install-recommends \
      ca-certificates \
 && test -s /etc/ssl/certs/ca-certificates.crt \
 && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      curl git gcc g++ make binutils libc6-dev pkg-config \
      python3 \
      python3-yaml=6.0.3-1build1 \
      unzip xz-utils zstd sudo \
      libclang-18-dev=1:18.1.8-20ubuntu8 \
 && dpkg-query -W gcc binutils libc6 libc6-dev libclang-18-dev python3 python3-yaml curl \
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
 && test "$(rustc --version | cut -d' ' -f2)" = 1.98.1 \
 && cargo --version && cargo fmt --version && cargo clippy --version

FROM rust-tools AS ci-base
# CI tools use pinned official binaries.
RUN case "$(dpkg --print-architecture)" in \
      amd64) archive=bun-linux-x64; checksum=36368faef7527875d5ffa52e53cd48021741f2a83eb6208a8dd64068d422a913 ;; \
      arm64) archive=bun-linux-aarch64; checksum=54328bbc2d9c8e0c9f892c544d66c57a83b84139e34909e5ee81758f1ac8fda7 ;; \
      *) exit 1 ;; \
    esac \
 && curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
      "https://github.com/oven-sh/bun/releases/download/bun-v1.4.2/$archive.zip" -o /tmp/bun.zip \
 && printf '%s  %s\n' "$checksum" /tmp/bun.zip | sha256sum --check --strict \
 && unzip -q /tmp/bun.zip -d /tmp/bun \
 && install -m 0755 "/tmp/bun/$archive/bun" /usr/local/bin/bun \
 && ln -s bun /usr/local/bin/bunx \
 && test "$(bun --version)" = 1.4.2 \
 && rm -rf /tmp/bun /tmp/bun.zip

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
FROM ci-base AS ci-web
USER root
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/playwright
RUN mkdir /tmp/ci-web \
 && cd /tmp/ci-web \
 && printf '%s\n' '{"private":true,"dependencies":{"@playwright/test":"1.63.0"}}' > package.json \
 && bun install --ignore-scripts \
 && test "$(bun --bun x --no-install playwright --version)" = 'Version 1.63.0' \
 && cat /etc/apt/sources.list.d/ubuntu.sources \
 && bun --bun x --no-install playwright install --with-deps chromium \
 && dpkg-query -W > /usr/local/share/ci-web-build-packages.txt \
 && cat /usr/local/share/ci-web-build-packages.txt \
 && rm -rf /tmp/ci-web /root/.bun /var/lib/apt/lists/*
USER 1000:1000
