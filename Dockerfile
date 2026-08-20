# syntax=docker/dockerfile:1
#
# Copyright 2026 Ronny Trommer <ronny@no42.org>
# SPDX-License-Identifier: Apache-2.0
#
# Multi-stage build producing a fully static `onmsctl` binary (musl libc, no
# dynamic linking) on a distroless base. The result has no shell and no package
# manager — invoke it as a pipeline step rather than a job container:
#
#   docker run --rm ghcr.io/no42-org/onmsctl:latest version
#
# Multi-arch (linux/amd64, linux/arm64) is built natively per platform under
# buildx/QEMU, so no cross-linker is required.

# ---- builder ---------------------------------------------------------------
# Pinned by digest (not just the tag) so a rebuilt image is byte-reproducible
# and cannot silently pull a re-pushed `1-bookworm`. Dependabot's docker
# ecosystem keeps the digest + tag comment current.
FROM rust:1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS builder

# musl-tools provides `musl-gcc`, used both as the C compiler for `ring`'s
# build script and as the linker for the *-musl target.
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# TARGETARCH is supplied by buildx (amd64 | arm64). Map it to the Rust musl
# triple, build a stripped release binary (see [profile.release] in
# Cargo.toml), and stage it at /out/onmsctl. The *-musl target links
# statically by default, so the artifact has no runtime libc dependency.
ARG TARGETARCH
RUN --mount=type=cache,id=cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-${TARGETARCH},target=/src/target \
    set -eux; \
    case "${TARGETARCH}" in \
      amd64) target=x86_64-unknown-linux-musl; \
             export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc ;; \
      arm64) target=aarch64-unknown-linux-musl; \
             export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc ;; \
      *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    export CC=musl-gcc; \
    rustup target add "${target}"; \
    cargo build --release --locked --bin onmsctl --target "${target}"; \
    install -D "target/${target}/release/onmsctl" /out/onmsctl

# ---- runtime ---------------------------------------------------------------
# distroless/static: no libc, no shell, runs as the bundled `nonroot` user
# (uid 65532). TLS roots are compiled into the binary (reqwest + webpki-roots),
# so no ca-certificates layer is needed. Pinned by digest for the same reason
# as the builder base; Dependabot keeps it current.
FROM gcr.io/distroless/static:nonroot@sha256:f7f8f729987ad0fdf6b05eeeae94b26e6a0f613bdf46feea7fc40f7bd72953e6

COPY --from=builder /out/onmsctl /usr/local/bin/onmsctl

# Static OCI metadata. The CI build augments these with version/revision
# labels derived by docker/metadata-action.
LABEL org.opencontainers.image.title="onmsctl" \
      org.opencontainers.image.description="Command-line interface for OpenNMS Horizon" \
      org.opencontainers.image.source="https://github.com/no42-org/onmsctl" \
      org.opencontainers.image.licenses="Apache-2.0"

ENTRYPOINT ["/usr/local/bin/onmsctl"]
