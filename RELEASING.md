# Releasing onmsctl

How to cut a release. The release workflow at
`.github/workflows/release.yml` does the heavy lifting; this document
covers the small set of decisions and human-driven steps.

## What ships per release

Each `v*.*.*` tag produces, via CI:

- **Binaries** for four targets, each as a single static executable:
  - `onmsctl-vX.Y.Z-x86_64-unknown-linux-gnu`
  - `onmsctl-vX.Y.Z-aarch64-unknown-linux-gnu`
  - `onmsctl-vX.Y.Z-x86_64-apple-darwin`
  - `onmsctl-vX.Y.Z-aarch64-apple-darwin`
- **SHA256 checksums** per binary (`*.sha256`) plus an aggregate
  `SHA256SUMS` covering binaries and SBOMs.
- **CycloneDX SBOM** (`onmsctl-vX.Y.Z-onmsctl.cdx.json`) for the
  shipped binary, spec version 1.5. Library and test crates are not
  shipped separately — their runtime deps are already in the binary's
  transitive tree.
- **Sigstore (cosign) keyless signatures** for every artifact:
  one `.sig` and one `.pem` per file.
- **GitHub Release**, created as a **draft** with auto-generated notes.
  Publishing is a deliberate human step — see *Publishing the draft*
  below.

Windows is not in the matrix yet; Windows operators build from source
per the README.

## Versioning

The workspace version lives in **one** place:
`[workspace.package].version` in `Cargo.toml`. All workspace crates
inherit it via `version.workspace = true`. There is no
per-crate version drift.

Semantic versioning:

- **`v0.x.y` (current):** Pre-stability. CLI flags, the config schema,
  and the `EventSource` YAML schema may break between minor versions.
  See the pre-stability notice in `README.md`.
- **`vX.Y.Z` (post-1.0):** Standard semver. Breaking surface changes
  require a major bump.

Tag format must match the strict regex
`^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$`
(enforced by the release workflow's `validate-tag` job). Examples:

- `v0.1.0` — stable release
- `v0.2.0-rc.1` — pre-release (any `-` suffix marks it prerelease)
- `v0.1.0+build.42` — build metadata (rare)

## Pre-release checklist

Before tagging, verify on `main`:

1. **CI is green** — `make verify` passes locally; the latest push to
   `main` has successful `verify` and `integration` workflow runs.
2. **Version bumped** — `Cargo.toml`'s `[workspace.package].version`
   matches the tag you're about to push (without the leading `v`).
   Cargo.lock updates from a `cargo check` after the bump are
   committed.
3. **README version references updated** — bump the version strings in
   `README.md` so the docs match the release:
   - the `VERSION=vX.Y.Z` value in the **Install** download example,
   - the `onmsctl X.Y.Z` sample output (and each capability line) under
     **Build from source**,
   - the image tags in the **Container image** section. Note these carry
     **no leading `v`** — the `vX.Y.Z` git tag publishes the image as
     `X.Y.Z` (plus `X.Y` and `latest`), per `docker/metadata-action`.
4. **Conventional Commits** — all commits since the previous tag
   follow `<type>(<scope>): <subject>` so `--generate-notes` seeds the
   draft cleanly. Breaking changes use `!` or a `BREAKING CHANGE:`
   footer.
5. **THIRD-PARTY-LICENSES.md** — regenerate if dependencies changed:
   `make licenses`. Commit any diff.
6. **OpenSpec is settled** — `openspec list` reports no active
   changes (or only changes intentionally deferred to a future
   release).

## Cutting the release

`main` is protected: it takes pull requests only, so the version bump
lands via a PR rather than a direct push.

```sh
# 1. Bump the version on a branch.
git checkout -b release/vX.Y.Z
$EDITOR Cargo.toml                              # update [workspace.package].version
$EDITOR README.md                               # bump the version refs (see checklist item 3)
cargo check --workspace                          # refresh Cargo.lock
git add Cargo.toml Cargo.lock README.md
git commit -s -m "chore(release): bump workspace version to vX.Y.Z"
git push -u origin release/vX.Y.Z
gh pr create --fill

# 2. Merge once the gate is green, then sync main.
gh pr merge --squash
git checkout main && git pull

# 3. Tag the merged bump commit and push the tag.
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin vX.Y.Z
```

The tag push triggers the release workflow. **Do not push the tag
before the version-bump commit lands on `main`** — the tag must point
at the commit whose `Cargo.toml` declares the matching version, or
the published binary's `onmsctl version` will lie.

## What CI does after the tag push

The `.github/workflows/release.yml` workflow runs these jobs in order:

1. **`validate-tag`** — checks the tag matches strict semver and
   determines whether it's a prerelease.
2. **`gate`** — calls the shared `.github/workflows/gates.yml`, the
   same `make verify` (fmt + clippy + build + test + deny) matrix that
   pull requests must pass. Nothing below it publishes if this is red.
3. **`build`** (matrix × 4 targets) — `make release-build
   TARGET=<triple>` for each target, stages the binary + SHA256
   under `dist/`.
4. **`sbom`** — `make sbom` runs `cargo about` and emits CycloneDX
   JSON files per workspace crate.
5. **`release`** — downloads every staged artifact, builds the
   aggregate `SHA256SUMS`, signs every file via `cosign sign-blob`
   (Sigstore keyless OIDC; no long-lived keys), and creates the
   GitHub Release **as a draft** with auto-generated notes.

`.github/workflows/docker.yml` runs in parallel off the same tag and is
gated on the same `gates.yml` before it pushes or signs the image.

The `release` job requests `id-token: write` only for itself — the
OIDC token surface stays narrow.

If the workflow fails partway, fix the underlying issue and re-push
the tag. The workflow's `concurrency` setting never cancels an
in-flight release: a re-pushed tag queues behind the running job
instead of racing it, and the release step reuses the existing draft
rather than failing on "already exists". Mint of new Sigstore certs on
each retry is expected.

## Publishing the draft

The workflow leaves the release in **draft**, so nothing is public
until you say so. Review, then publish:

```sh
VERSION=vX.Y.Z

# Confirm every expected asset is attached (4 binaries + 4 .sha256,
# SHA256SUMS, the SBOM, and a .sig/.pem for each signed file).
gh release view "${VERSION}" --json assets --jq '.assets[].name' | sort

# Replace the auto-generated notes with curated ones, and publish.
$EDITOR notes.md
gh release edit "${VERSION}" --notes-file notes.md --draft=false
```

Publish promptly: `docker.yml` pushes the container tags (including
`latest` for a stable tag) as soon as it finishes, so a long-lived
draft leaves GHCR ahead of the GitHub Release. Prereleases keep their
`--prerelease` flag and never move `latest`.

## Verifying a published release

After CI completes (≈8 minutes), verify against the GitHub Release page:

```sh
VERSION=vX.Y.Z
TARGET=x86_64-apple-darwin  # or another row from the README matrix

curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.sha256
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.sig
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.pem

# Checksum
shasum -a 256 -c onmsctl-${VERSION}-${TARGET}.sha256

# Cosign — ties the binary to a specific release.yml run on this repo
cosign verify-blob \
  --certificate-identity-regexp "^https://github.com/no42-org/onmsctl/.github/workflows/release.yml@" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate onmsctl-${VERSION}-${TARGET}.pem \
  --signature  onmsctl-${VERSION}-${TARGET}.sig \
  onmsctl-${VERSION}-${TARGET}

# Smoke test
chmod +x onmsctl-${VERSION}-${TARGET}
./onmsctl-${VERSION}-${TARGET} version  # must print exactly VERSION (no leading v)
```

If `onmsctl version` doesn't match the tag, the version-bump commit was
missed or the tag points at the wrong commit. See **Repairing a bad
release** below.

## Pre-releases

Any tag containing `-` is a prerelease. The release workflow detects
this and sets `prerelease: true` on the GitHub Release. Use this for
release candidates, betas, and alpha previews:

```sh
git tag -a v0.2.0-rc.1 -m "v0.2.0 release candidate 1"
git push origin v0.2.0-rc.1
```

Prereleases ship the same artifacts as stable releases but won't
appear as "Latest release" on the GitHub Releases page.

## Hotfix flow

For a critical fix on an already-released version (e.g. `v0.1.0` is
broken in the wild, fix is small, full `main` isn't ready):

```sh
# 1. Branch from the bad tag.
git checkout -b hotfix/v0.1.1 v0.1.0

# 2. Land the fix (cherry-pick from main if it's already there).
git cherry-pick <sha>

# 3. Bump version, commit, tag.
$EDITOR Cargo.toml                              # 0.1.0 → 0.1.1
cargo check --workspace
git add Cargo.toml Cargo.lock
git commit -m "chore(release): bump to v0.1.1"
git tag -a v0.1.1 -m "Hotfix v0.1.1"

# 4. Push branch + tag. The release workflow ships from the tag,
#    not main. Merge the hotfix branch back into main afterward.
git push origin hotfix/v0.1.1 v0.1.1
```

## Repairing a bad release

A release that compiled and published but has a wrong binary
(version mismatch, missing fix, etc.) is best handled by **yanking**
plus a follow-up version:

1. On the GitHub Release page, mark the bad release as
   "Pre-release" so it stops showing as "Latest" — do NOT delete it
   if anyone might have downloaded the artifacts (signature
   verifications depend on the artifacts staying reachable).
2. Edit the release notes to point at the replacement version.
3. Cut a new patch release with the fix.

Avoid force-deleting tags. The Sigstore transparency log retains the
signing record independently of the tag, so removing a tag doesn't
"unrelease" it.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Workflow fails at 4-6s with no logs | GitHub Actions billing block on the org | Resolve under `Settings → Billing & plans` on the org account; re-push the tag |
| `validate-tag` rejects the tag | Tag doesn't match strict semver | Delete the local + remote tag, retag with a valid name |
| `build` fails on `aarch64-unknown-linux-gnu` | Cross-compile linker missing in the runner | The matrix installs `gcc-aarch64-linux-gnu` via apt; check the apt step for transient mirror failures |
| `release` job fails at `cosign sign-blob` | Sigstore Rekor outage (transient) | Re-run the failing job from the Actions UI |
| `onmsctl version` doesn't match tag | Tag points at a commit before the version bump | Cut a new patch release |
