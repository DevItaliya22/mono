# syntax=docker/dockerfile:1
# SPDX-License-Identifier: MPL-2.0

# The koru-relay image: the upstream iroh-relay server, configured by this
# crate. Build it from the repository root:
#
#   docker build -f apps/koru-relay/Dockerfile -t koru-relay:dev .
#
# or, from a checkout that carries this repository as a subdirectory (Koru
# Works' deployment has it at oss/), with that subdirectory as the context:
#
#   docker build -f oss/apps/koru-relay/Dockerfile -t koru-relay:dev oss
#
# BuildKit reads Dockerfile.dockerignore beside this file, which keeps the
# context to what the build reads instead of the whole tree.
#
# The binary is compiled inside the build stage, so the host needs Docker and
# nothing else: no Rust toolchain, no cross-compilation, no prebuilt artifact.
# Where the image runs, and with which configuration, is the deployment's
# business; this file only makes the image.

# The toolchain rust-toolchain.toml pins. Bump the two together: rustup reads
# the copied file below and installs that toolchain if this image differs, so
# a stale tag costs a download, never a different compiler.
FROM rust:1.99.0-slim-trixie AS builder

# What scripts/setup/fetch-third-party.sh uses: curl to download, git to
# apply the patches. (tar and perl are in the base image.)
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cargo resolves the whole workspace even when building one package, so every
# member's manifest and sources must be here, and the patched third-party
# crates the workspace names by path. Those are not committed: the fetch
# below downloads them, refuses anything that does not match its pin in
# third-party/fetch.manifest, and applies Koru's patches.
#
# Deliberately not copied: `.cargo/config.toml`. It requires sccache and
# configures the Windows linker, which are properties of a development
# machine, not of this build. The container builds with the stock toolchain.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY scripts/setup/fetch-third-party.sh scripts/setup/
COPY third-party/ third-party/
COPY crates/ crates/
COPY apps/ apps/
RUN bash scripts/setup/fetch-third-party.sh

# `--locked`: the committed Cargo.lock is the reproducibility boundary. A
# drifted lockfile fails the build rather than resolving something newer.
RUN cargo build -p koru-relay --release --locked

# Prove the artifact runs on this libc before it is shipped anywhere.
RUN target/release/koru-relay --version

# ---------------------------------------------------------------------------
# Runtime: Debian slim, not distroless. The server needs CA roots (the ACME
# client must trust Let's Encrypt's API; rustls needs no OpenSSL, but trust
# anchors still have to come from somewhere), and `docker exec` with a real
# shell is worth more to whoever operates a relay than distroless's marginal
# surface reduction. Same Debian release as the builder, so the glibc the
# binary linked against is the glibc it runs on.
FROM debian:trixie-slim

# A deployment passes these; the defaults only mark an untagged local build.
ARG KORU_RELAY_VERSION=dev
ARG KORU_RELAY_GIT_SHA=unknown
LABEL org.opencontainers.image.title="koru-relay" \
      org.opencontainers.image.description="The Koru iroh relay (apps/koru-relay)" \
      org.opencontainers.image.source="https://github.com/koruworks/mono" \
      org.opencontainers.image.licenses="MPL-2.0" \
      org.opencontainers.image.version="${KORU_RELAY_VERSION}" \
      org.opencontainers.image.revision="${KORU_RELAY_GIT_SHA}"

# ca-certificates: trust anchors for the ACME directory (see above).
# The server runs as a dedicated system user; /var/lib/koru-relay/acme is
# created owned by it so that an *empty* named volume mounted there inherits
# this ownership on first use (Docker copies the image path's content and
# ownership into a fresh named volume) and the non-root server can write
# its certificate cache.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system koru-relay \
 && useradd --system --gid koru-relay --home-dir /var/lib/koru-relay \
      --shell /usr/sbin/nologin koru-relay \
 && install -d -o koru-relay -g koru-relay /var/lib/koru-relay/acme

COPY --from=builder /build/target/release/koru-relay /usr/local/bin/koru-relay

# Non-root. Binding 80/443 below 1024 works because the container's
# net.ipv4.ip_unprivileged_port_start is 0 (Docker's default since 20.10; a
# compose file should still set it, so the property is declared, not assumed).
USER koru-relay

# 80/tcp captive-portal probe, 443/tcp relay + /healthz + ACME TLS-ALPN-01,
# 7842/udp QUIC address discovery, 9090/tcp Prometheus metrics (private:
# publish it on the host loopback only).
EXPOSE 80/tcp 443/tcp 7842/udp 9090/tcp

# ENTRYPOINT is the binary and CMD only the default arguments, so a monitor
# can run `docker run <image> probe <url>` (same image, same code, different
# arguments) without touching the entrypoint.
ENTRYPOINT ["koru-relay"]
CMD ["--config", "/etc/koru-relay/config.toml"]
