---
title: Install
description: Install onmsctl from a pre-compiled binary, a container image, or build it from source.
---

## Prerequisites

- A reachable OpenNMS Horizon instance and its base URL (e.g. `https://horizon.dev.lab/opennms`).
- A Horizon user with the rights for what you intend to do.
  Read verbs need read access.
  `apply` and other write verbs need a role that can mutate the relevant resource (admin for IAM).
- To build from source: the Rust toolchain pinned in `rust-toolchain.toml`.

## Pre-compiled binary (recommended)

Pre-compiled binaries are published as GitHub Releases for every `v*.*.*` tag, with per-binary SHA256 checksums, an aggregate `SHA256SUMS`, and Sigstore (cosign) keyless signatures + certificates for every asset.

| Target | Asset suffix |
|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` |
| macOS x86_64 (Intel) | `x86_64-apple-darwin` |
| macOS aarch64 (Apple Silicon) | `aarch64-apple-darwin` |

Windows is not yet in the release matrix; Windows users build from source.

```sh
# Resolve the newest release tag (follows the /releases/latest redirect;
# needs only curl). Set VERSION=vX.Y.Z by hand instead if you prefer to pin.
VERSION=$(basename "$(curl -fsSLo /dev/null -w '%{url_effective}' \
  https://github.com/no42-org/onmsctl/releases/latest)")
TARGET=x86_64-apple-darwin   # or one of the rows above

curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.sha256
shasum -a 256 -c onmsctl-${VERSION}-${TARGET}.sha256
chmod +x onmsctl-${VERSION}-${TARGET}
sudo mv onmsctl-${VERSION}-${TARGET} /usr/local/bin/onmsctl
onmsctl version
```

**Verify the cosign signature (recommended).**
This ties the binary to a specific GitHub Actions run on this repo, with no long-lived key:

```sh
cosign verify-blob \
  --certificate-identity-regexp "^https://github.com/no42-org/onmsctl/.github/workflows/release.yml@" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate onmsctl-${VERSION}-${TARGET}.pem \
  --signature  onmsctl-${VERSION}-${TARGET}.sig \
  onmsctl-${VERSION}-${TARGET}
```

**Verify build provenance (optional).**
Every binary also carries a GitHub-issued SLSA attestation proving it was built by this repo's release workflow from a specific commit.
Where the cosign check above proves *who signed it*, this proves *how it was built*:

```sh
gh attestation verify onmsctl-${VERSION}-${TARGET} --repo no42-org/onmsctl
```

Binaries are Sigstore-signed but not Apple-notarized; on macOS, clear the quarantine flag once with `xattr -d com.apple.quarantine /usr/local/bin/onmsctl` (or approve via System Settings → Privacy & Security).

## Build from source

Requires the toolchain pinned in `rust-toolchain.toml` (currently Rust 1.95):

```sh
git clone https://github.com/no42-org/onmsctl && cd onmsctl
make build                              # debug → target/debug/onmsctl
cargo install --path crates/onmsctl     # → ~/.cargo/bin
```

`onmsctl` is one binary statically linking every capability crate.
`onmsctl version` prints the binary version alongside each linked capability:

```
onmsctl 0.4.7
capabilities:
  - eventconf 0.4.7
  - provisioning 0.4.7
  - iam 0.4.7
  - snmp 0.4.7
  - maintenance 0.4.7
  - datacollection 0.4.7
  - business-service 0.4.7
```

The capability list grows as the binary links new capability crates.

## Container image

A multi-arch (`linux/amd64`, `linux/arm64`) **distroless** image is published to GHCR for every `v*.*.*` tag at `ghcr.io/no42-org/onmsctl`.
It's a single static binary on `gcr.io/distroless/static` with no shell and no package manager, running as the non-root user `65532`, so it's small and has a minimal attack surface for CI/CD pipelines.
Each release publishes the exact version (`0.4.7`), the rolling `MAJOR.MINOR` tag (`0.4`), and `latest` (the newest non-prerelease).
Image tags carry no leading `v` (the `v0.4.7` git tag publishes as `0.4.7`):

```sh
docker run --rm ghcr.io/no42-org/onmsctl:latest version
```

Mount your config and manifests to run a declarative apply as a pipeline step (the entrypoint is `onmsctl`, so pass subcommands directly).
`apply` resolves a config file that defines the context (server URL + user).
Point `ONMSCTL_CONFIG` at the mounted file and supply the password at runtime via `ONMS_PASSWORD`:

```sh
docker run --rm \
  -e ONMSCTL_CONFIG=/work/onmsctl.yaml \
  -e ONMS_PASSWORD \
  -v "$PWD:/work:ro" -w /work \
  ghcr.io/no42-org/onmsctl:latest apply -f requisition.yaml
```

`ONMS_URL`, `ONMS_USER`, and `ONMS_TOKEN` are also honored as overrides on top of the resolved context.
See [Configure a context](configure-context.md) for the config-file format.

Because the image is distroless it has **no shell**.
Use it as a `docker run` step rather than as a GitLab `image:` / GitHub `container:` job that expects to run `before_script` shell commands.

Images carry build provenance + an SBOM and are Sigstore-signed (keyless).
Verify the signature ties the image to a build of this repo's `docker.yml` workflow:

```sh
cosign verify \
  --certificate-identity-regexp "^https://github.com/no42-org/onmsctl/.github/workflows/docker.yml@" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  ghcr.io/no42-org/onmsctl:latest
```

The image also carries a GitHub-issued SLSA attestation, verifiable by digest:

```sh
gh attestation verify oci://ghcr.io/no42-org/onmsctl:latest --repo no42-org/onmsctl
```

> **Bleeding edge.**
> `ghcr.io/no42-org/onmsctl:rc` tracks the tip of `main`, rebuilt and overwritten on every merge.
> It's signed like a release, but explicitly unstable.
> A matching rolling `preview` prerelease carries the binaries.
> Use a `vX.Y.Z` tag for anything real.
> See [RELEASING.md](https://github.com/no42-org/onmsctl/blob/main/RELEASING.md#preview-builds-automatic-every-merge-to-main).

Build it locally for your host architecture with `make docker` (produces `onmsctl:dev`).
