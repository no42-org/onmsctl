---
title: Install
description: Install onmsctl from a pre-compiled binary, a container image, or build it from source.
---

## Check the prerequisites

- A reachable OpenNMS Horizon instance and its base URL (e.g. `https://horizon.example.com/opennms`).
- A Horizon user with the rights for what you intend to do.
  Read verbs need read access.
  `apply` and other write verbs need a role that can mutate the relevant resource (admin for IAM).
- To build from source: the Rust toolchain pinned in `rust-toolchain.toml`.

## Install a pre-compiled binary

Pre-compiled binaries are the recommended install.
They are published as GitHub Releases for every `v*.*.*` tag.
Each binary comes with a SHA256 checksum, a Sigstore (cosign) keyless signature and a certificate.
An aggregate `SHA256SUMS` covers all binaries.

| Target | Asset suffix |
|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` |
| macOS x86_64 (Intel) | `x86_64-apple-darwin` |
| macOS aarch64 (Apple Silicon) | `aarch64-apple-darwin` |

Windows is not yet in the release matrix.
Windows users build from source.

### Download and verify the checksum

Set **`TARGET`** to one of the asset suffixes above.
The block downloads the binary, its checksum, its signature and its certificate:

```sh
# Resolve the newest release tag (follows the /releases/latest redirect;
# needs only curl). Set VERSION=vX.Y.Z by hand instead if you prefer to pin.
VERSION=$(basename "$(curl -fsSLo /dev/null -w '%{url_effective}' \
  https://github.com/no42-org/onmsctl/releases/latest)")
TARGET=x86_64-apple-darwin   # or one of the rows above

curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.sha256
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.sig
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.pem
shasum -a 256 -c onmsctl-${VERSION}-${TARGET}.sha256
chmod +x onmsctl-${VERSION}-${TARGET}
sudo mv onmsctl-${VERSION}-${TARGET} /usr/local/bin/onmsctl
onmsctl version
```

### Verify the cosign signature

This check is recommended.
It ties the binary to a specific GitHub Actions run on this repo, with no long-lived key.
Run it in the download directory, where the `.sig` and `.pem` files are.
It checks the installed binary, since the download block moved it to `/usr/local/bin/onmsctl`:

```sh
cosign verify-blob \
  --certificate-identity-regexp "^https://github.com/no42-org/onmsctl/.github/workflows/release.yml@" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate onmsctl-${VERSION}-${TARGET}.pem \
  --signature  onmsctl-${VERSION}-${TARGET}.sig \
  /usr/local/bin/onmsctl
```

### Verify the build provenance

This check is optional.
Every binary carries a GitHub-issued SLSA attestation.
It proves the binary was built by this repo's release workflow from a specific commit.
The cosign check proves *who signed it*.
This check proves *how it was built*:

```sh
gh attestation verify /usr/local/bin/onmsctl --repo no42-org/onmsctl
```

### Clear the macOS quarantine flag

Binaries are Sigstore-signed but not Apple-notarized.
This matters only if you downloaded the binary with a browser, which sets the quarantine flag.
`curl` does not set it.
Clear the flag once with **`xattr -d com.apple.quarantine /usr/local/bin/onmsctl`**, or approve the binary in System Settings → Privacy & Security.

## Build from source

Building requires the toolchain pinned in **`rust-toolchain.toml`** (currently Rust 1.95):

```sh
git clone https://github.com/no42-org/onmsctl && cd onmsctl
make build                              # debug → target/debug/onmsctl
cargo build --release                   # → target/release/onmsctl
cargo install --path crates/onmsctl     # → ~/.cargo/bin
```

`onmsctl` is one binary that statically links every capability crate.
**`onmsctl version`** prints the binary version and each linked capability:

```sh
onmsctl version
```

Expected output:

```text
onmsctl 0.6.0
capabilities:
  - eventconf 0.6.0
  - provisioning 0.6.0
  - iam 0.6.0
  - snmp 0.6.0
  - maintenance 0.6.0
  - datacollection 0.6.0
  - business-service 0.6.0
  - thresholding 0.6.0
  - graph 0.6.0
```

## Run the container image

A multi-arch (`linux/amd64`, `linux/arm64`) distroless image is published to GHCR for every `v*.*.*` tag at **`ghcr.io/no42-org/onmsctl`**.
It is a single static binary on `gcr.io/distroless/static` with no shell and no package manager.
It runs as the non-root user `65532`.
Each release publishes the exact version (`0.6.0`), the rolling `MAJOR.MINOR` tag (`0.5`), and `latest` (the newest non-prerelease).
Image tags carry no leading `v`: the `v0.6.0` git tag publishes as `0.6.0`.

```sh
docker run --rm ghcr.io/no42-org/onmsctl:latest version
```

The output has the same shape as `onmsctl version` under [Build from source](#build-from-source).

### Run apply as a pipeline step

The entrypoint is `onmsctl`, so pass subcommands directly.
Mount your config and manifests into the container.
`apply` needs a config file that defines the context (server URL and user).
Point **`ONMSCTL_CONFIG`** at the mounted file and supply the password at runtime with **`ONMS_PASSWORD`**:

```sh
docker run --rm \
  -e ONMSCTL_CONFIG=/work/onmsctl.yaml \
  -e ONMS_PASSWORD \
  -v "$PWD:/work:ro" -w /work \
  ghcr.io/no42-org/onmsctl:latest apply -f requisition.yaml
```

`ONMS_URL`, `ONMS_USER`, and `ONMS_TOKEN` are also honored as overrides on top of the resolved context.
See [Configure a context](configure-context.md) for the config-file format.

The image is distroless, so it has **no shell**.
Use it as a `docker run` step.
Do not use it as a GitLab `image:` or GitHub `container:` job that runs `before_script` shell commands.

### Verify the image signature

Images carry build provenance and an SBOM, and are Sigstore-signed (keyless).
This check ties the image to a build of this repo's `docker.yml` workflow:

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

### Try a preview build

**`ghcr.io/no42-org/onmsctl:rc`** tracks the tip of `main`.
It is rebuilt and overwritten on every merge.
It is signed like a release, but it is unstable.
A matching rolling `preview` prerelease carries the binaries.
Use a `vX.Y.Z` tag for anything real.
See [RELEASING.md](https://github.com/no42-org/onmsctl/blob/main/RELEASING.md#preview-builds-automatic-every-merge-to-main).

### Build the image locally

Run **`make docker`** to build an image for your host architecture.
It produces `onmsctl:dev`.
