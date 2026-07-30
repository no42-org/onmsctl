# onmsctl

[![verify](https://github.com/no42-org/onmsctl/actions/workflows/verify.yml/badge.svg?branch=main)](https://github.com/no42-org/onmsctl/actions/workflows/verify.yml)
[![CodeQL](https://github.com/no42-org/onmsctl/actions/workflows/codeql.yml/badge.svg?branch=main)](https://github.com/no42-org/onmsctl/actions/workflows/codeql.yml)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/no42-org/onmsctl/badge)](https://scorecard.dev/viewer/?uri=github.com/no42-org/onmsctl)
[![latest release](https://img.shields.io/github/v/release/no42-org/onmsctl?sort=semver)](https://github.com/no42-org/onmsctl/releases/latest)
[![license](https://img.shields.io/github/license/no42-org/onmsctl)](LICENSE)

A `kubectl`-style command-line interface for [OpenNMS Horizon][horizon]. One
declarative entrypoint — `onmsctl apply -f` — peeks each YAML document's `kind`
and routes it to the right handler, so users, event sources, SNMP config,
requisitions, maintenance windows, and data-collection sources all reconcile
through a single command. XML→YAML migrators bring legacy eventconf and
`provision.pl`-shape files into the loop; reads, explicit deletes, and `convert`
stay imperative.

[horizon]: https://www.opennms.com/horizon/

> **New to onmsctl?** Start with the [Quick Start](docs/quickstart.md) — install,
> configure a context, and run your first `apply` in a few minutes.

> **Pre-stability notice.** `v0.x.y` releases may break CLI flags, the config
> schema, and the `EventSource` YAML schema between minor versions. Surfaces
> stabilize at `v1.0.0`.

## Contents

- [Install](#install) · [Container image](#container-image) · [Configure a context](#configure-a-context)
- [Declarative apply](#declarative-apply-onmsctl-apply--f) — the apply model
- **Capabilities:** [Event configuration](#event-configuration-kind-eventsource) ·
  [Provisioning](#provisioning-kind-requisition) ·
  [Users and roles](#users-and-roles-kind-user) ·
  [SNMP configuration](#snmp-configuration-kind-snmpconfig) ·
  [Maintenance windows](#maintenance-windows-kind-maintenance) ·
  [SNMP data collection](#snmp-data-collection-kind-datacollectionsource) ·
  [Business services](#business-services-kind-businessservice)
- [Conventions & tooling](#conventions--tooling) · [Server compatibility](#server-compatibility)
- [Migration](docs/migration.md) · [EventSource reference](docs/eventsource-reference.md)

---

## Install

Pre-compiled binaries are published as GitHub Releases for every `v*.*.*` tag,
with per-binary SHA256 checksums, an aggregate `SHA256SUMS`, and Sigstore
(cosign) keyless signatures + certificates for every asset.

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

**Verify the cosign signature (recommended)** — ties the binary to a specific
GitHub Actions run on this repo, with no long-lived key:

```sh
cosign verify-blob \
  --certificate-identity-regexp "^https://github.com/no42-org/onmsctl/.github/workflows/release.yml@" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate onmsctl-${VERSION}-${TARGET}.pem \
  --signature  onmsctl-${VERSION}-${TARGET}.sig \
  onmsctl-${VERSION}-${TARGET}
```

**Verify build provenance (optional)** — every binary also carries a
GitHub-issued SLSA attestation proving it was built by this repo's release
workflow from a specific commit. Where the cosign check above proves *who
signed it*, this proves *how it was built*:

```sh
gh attestation verify onmsctl-${VERSION}-${TARGET} --repo no42-org/onmsctl
```

Binaries are Sigstore-signed but not Apple-notarized; on macOS, clear the
quarantine flag once with `xattr -d com.apple.quarantine /usr/local/bin/onmsctl`
(or approve via System Settings → Privacy & Security).

### Build from source

Requires the toolchain pinned in `rust-toolchain.toml` (currently Rust 1.95):

```sh
git clone https://github.com/no42-org/onmsctl && cd onmsctl
make build                              # debug → target/debug/onmsctl
cargo install --path crates/onmsctl     # → ~/.cargo/bin
```

`onmsctl` is one binary statically linking every capability crate. `onmsctl
version` prints the binary version alongside each linked capability:

```
onmsctl 0.4.4
capabilities:
  - eventconf 0.4.4
  - provisioning 0.4.4
  - iam 0.4.4
  - snmp 0.4.4
  - maintenance 0.4.4
  - datacollection 0.4.4
  - business-service 0.4.4
```

---

## Container image

A multi-arch (`linux/amd64`, `linux/arm64`) **distroless** image is published to
GHCR for every `v*.*.*` tag at `ghcr.io/no42-org/onmsctl`. It's a single static
binary on `gcr.io/distroless/static` — no shell, no package manager, running as
the non-root user `65532` — so it's small and has a minimal attack surface for
CI/CD pipelines. Each release publishes the exact version (`0.4.4`), the
rolling `MAJOR.MINOR` tag (`0.4`), and `latest` (the newest non-prerelease).
Image tags carry no leading `v` (the `v0.4.4` git tag publishes as `0.4.4`):

```sh
docker run --rm ghcr.io/no42-org/onmsctl:latest version
```

Mount your config and manifests to run a declarative apply as a pipeline step
(the entrypoint is `onmsctl`, so pass subcommands directly). `apply` resolves a
config file that defines the context (server URL + user); point `ONMSCTL_CONFIG`
at the mounted file and supply the password at runtime via `ONMS_PASSWORD`:

```sh
docker run --rm \
  -e ONMSCTL_CONFIG=/work/onmsctl.yaml \
  -e ONMS_PASSWORD \
  -v "$PWD:/work:ro" -w /work \
  ghcr.io/no42-org/onmsctl:latest apply -f requisition.yaml
```

`ONMS_URL`, `ONMS_USER`, and `ONMS_TOKEN` are also honored as overrides on top
of the resolved context. See [Configure a context](#configure-a-context) for the
config-file format.

Because the image is distroless it has **no shell** — use it as a `docker run`
step rather than as a GitLab `image:` / GitHub `container:` job that expects to
run `before_script` shell commands.

Images carry build provenance + an SBOM and are Sigstore-signed (keyless). Verify
the signature ties the image to a build of this repo's `docker.yml` workflow:

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

> **Bleeding edge.** `ghcr.io/no42-org/onmsctl:rc` tracks the tip of `main`,
> rebuilt and overwritten on every merge — signed like a release, but
> explicitly unstable. A matching rolling `preview` prerelease carries the
> binaries. Use a `vX.Y.Z` tag for anything real. See
> [RELEASING.md](RELEASING.md#preview-builds-automatic-every-merge-to-main).

Build it locally for your host architecture with `make docker` (produces
`onmsctl:dev`).

---

## Configure a context

kubectl pattern: one config file, one or more named contexts, one active context.

| OS | Default path |
|---|---|
| Linux | `$XDG_CONFIG_HOME/onmsctl/config.yaml` (typically `~/.config/onmsctl/config.yaml`) |
| macOS | `~/Library/Application Support/org.no42-org.onmsctl/config.yaml` |
| Windows | `%APPDATA%\no42-org\onmsctl\config\config.yaml` |

Override with `--config <path>` or `$ONMSCTL_CONFIG`.

```yaml
current-context: dev
contexts:
  - name: dev
    server:
      url: https://horizon.dev.lab/opennms
    auth:
      basic:
        username: admin
        password: admin      # don't commit inline secrets — see Credentials
```

```sh
onmsctl config view                  # current config (inline secrets redacted)
onmsctl config use-context staging   # atomic switch of the active context
```

**Credentials.** `auth.basic` / `auth.bearer` take exactly one source: inline
(`password`/`token`), a file (`password-file`/`token-file`, mode `0600`), or the
OS `keyring` (macOS Keychain / Windows Credential Manager built-in; Linux needs a
rebuild with `--features keyring/sync-secret-service`). Resolution at request
time: `env ($ONMS_PASSWORD / $ONMS_TOKEN) > keyring > file > inline`.

**Override precedence:** `flags (--url, --user, --context) > env (ONMS_URL,
ONMS_USER, ONMSCTL_CONTEXT) > active context > built-in default`.

Full credential + context walkthrough: [Quick Start](docs/quickstart.md#3-configure-a-context).

---

## Declarative apply (`onmsctl apply -f`)

`onmsctl apply -f <file|dir|glob>` is the single declarative mutation entrypoint.
It peeks each YAML document's `kind` and routes it to the registered handler —
there is no per-capability apply verb. Recognized kinds:

| `kind` | `apiVersion` | Reconciles |
|---|---|---|
| `User` | `onmsctl.no42.org/v1alpha1` | Horizon users + roles |
| `EventSource` | `eventconf.opennms.org/v1` | event configuration sources |
| `SnmpConfig` | `snmp.opennms.org/v1` | SNMP agent + trap-daemon config (singleton) |
| `Requisition` | `provisioning.opennms.org/v1` | provisioning requisitions |
| `Maintenance` | `maintenance.opennms.org/v1` | scheduled-outage maintenance windows |
| `DataCollectionSource` | `datacollection.opennms.org/v1` | SNMP data-collection sources |
| `BusinessService` | `bsm.opennms.org/v1` | Business Service Monitoring (BSM) services + edges |

A single file may hold many `---`-separated documents, and a directory can mix
all kinds.

**Plan → gate → execute.** Every document is planned first. If *any* fails to
plan — unknown `kind`, duplicate `metadata.name`, parse error — the whole apply
**aborts before any mutation**. Once the gate passes, documents execute in a
static precedence order so dependencies settle first:

```
User (100) → EventSource (200) → SnmpConfig (250) → Requisition (300) → Maintenance (350) → DataCollectionSource (375) → BusinessService (400)
```

Each document yields one `ApplyOutcome` row, rendered through `-o table|yaml|json`:

```text
kind         name       action  status     message
Requisition  acme-prod  create  Skipped    dry-run: would create
Requisition  site-b     none    Unchanged  in sync
```

```sh
onmsctl apply -f users.yaml                       # single file
onmsctl apply -f ./desired-state/                 # directory (mixed kinds)
onmsctl apply -f ./desired-state/ -R              # recurse into subdirs
onmsctl apply -f 'sources/cisco-*.yaml'           # glob (quote it)
```

| Flag | Behavior |
|---|---|
| `--dry-run` | Plan only; zero mutating HTTP. Classifies as a Read, so `--read-only` contexts may run it. |
| `--diff` | Render each kind-bucket's diff to stderr (stdout stays clean for `-o json/yaml`). |
| `--continue-on-error` (alias `--keep-going`) | Keep applying after a failing document. Default is stop-on-error. |
| `-R` / `--recursive` | Recurse into subdirectories (off by default). |

**Exit codes:** `0` all applied/unchanged; `1` any document failed (incl. a
plan-gate failure); `2` usage error. Full table under
[Conventions & tooling](#exit-codes). The imperative mutators that predated this
model are gone — see [Migration](docs/migration.md#removed-imperative-verbs--onmsctl-apply--f).

---

## Event configuration (`kind: EventSource`)

Keep event configuration in git as YAML and push it to Horizon declaratively.
The loop: `event-source convert` brings existing XML in, `onmsctl apply -f` ships
edits out, `event-source export` snapshots the server back to YAML.

```sh
onmsctl event-source convert /opt/opennms/etc/events/cisco.foo.events.xml   # XML → YAML
onmsctl apply -f cisco.foo.yaml --diff                                       # push (diff to stderr)
onmsctl event-source export cisco.foo --out ./sources/                       # server → YAML
```

`metadata.name` becomes the server's stored source name verbatim; Horizon derives
`vendor` as the prefix before the first `.` (`cisco.foo` → vendor `cisco`). `apply`
is idempotent (Horizon's upsert replaces events under an existing basename).
Bulk export is continue-on-error and runs each source through the `convert`
migrator, so the same `EC###` findings apply.

Read/raw verbs stay imperative — `event-source list|get|names|names-and-ids`,
`event list` (incl. cross-source `--uei`/`--vendor`), and the raw-XML
`event-source upload|download` round-trip (use these for full fidelity when a
source carries fields the YAML model doesn't represent yet).

The [`examples/`](examples/README.md) directory ships `event-source-{minimal,full,severities,disabled}.yaml`.
See the [Quick Start](docs/quickstart.md#6-gitops-for-event-configuration) for the
walkthrough and the [EventSource reference](docs/eventsource-reference.md) for the
field-by-field schema and the `event-source convert` finding catalog.

---

## Provisioning (`kind: Requisition`)

Manage Horizon requisitions (the `provision.pl`-shape data) declaratively from
git: `requisition convert` brings existing XML in, `apply` ships edits out,
`requisition import` / `status` cover the lifecycle.

```sh
onmsctl requisition convert --from etc/imports/ --foreign-sources-dir etc/foreign-sources/ --out yaml/
onmsctl apply -f acme-prod.yaml --dry-run --diff      # preview
onmsctl apply -f acme-prod.yaml                        # apply (computes the diff, triggers import)
onmsctl requisition import acme-prod --wait --timeout 5m   # block on the async import
```

The apply path computes a three-level diff (canonical-bytes / per-node / per-leaf),
**auto-decides `rescanExisting`** from the scan-relevance of what changed, writes
the foreign-source + requisition, and triggers the import. Import completion is
asynchronous — block with `import --wait` or poll `requisition status <fs>`.

- **Pinned vs portable `foreignSource`.** Include `spec.foreignSource` (detectors +
  policies) and the YAML owns both the requisition and its foreign-source. Omit it
  and the requisition inherits Horizon's default FS — and if a custom FS existed
  for that name, `apply` **deletes it** (the `--diff` enumerates the displaced
  detectors/policies). The [`examples/requisition-acme-prod.yaml`](examples/requisition-acme-prod.yaml)
  fixture shows the pinned style with every modeled field.
- **Unmodeled XML is preserved** under `metadata.x-onmsctl-unmodeled`, so a
  `convert → apply → export → re-apply` round-trip keeps server-side fields onmsctl
  doesn't model yet. The annotation is stripped before the diff and the wire body,
  so it never changes apply outcome.

Lifecycle/read verbs: `requisition list|status|import|export|delete` and the
`node|interface|service|category|asset list/get` reads. `requisition delete <fs>`
requires `--yes` (it purges both pending and deployed snapshots).

[Quick Start §7](docs/quickstart.md#7-gitops-for-provisioning-requisitions) ·
[`provision.pl` migration](docs/migration.md#provisionpl-verb--onmsctl).

---

## Users and roles (`kind: User`)

Manage Horizon users and roles declaratively (`kind: User`) and imperatively via
the read / rotate / delete verbs. Targets the `users` REST surface on Horizon 35+.

```sh
onmsctl iam whoami                                    # the calling user
onmsctl apply -f ./users/ --dry-run --diff            # preview
onmsctl apply -f ./users/                             # apply
onmsctl iam user list|get|export                      # reads
onmsctl iam user delete alice --yes
printf %s "$NEW_PW" | onmsctl iam user set-password alice --password-stdin
```

- **Roles reconcile as a set** — `roles: [B, C]` against a server holding `[A, B]`
  grants `C`, revokes `A`, keeps `B`. Omitted scalar fields never clear the server
  value (merge, not replace).
- **`passwordRef` — passwords are create-only and never inline.** A literal
  `password:` is rejected at parse (`PR-IAM-001`); reference an external secret
  (`fromFile`/`fromEnv`/`fromKeyring`). Honored on Create only — `apply` never
  rotates (it can't read the current password to diff); use `iam user set-password`.
- **Lockout protection.** Apply refuses a plan that would empty a protected role
  (`IAM-001`, exit 13; override per-context with `allow-admin-lockout: true`) or
  strip/delete the calling user's own protected role (`IAM-002`, exit 14, no
  override). If `whoami` is unavailable for a self-affecting change, it refuses
  (exit 15) rather than skip the check.
- `dutySchedule` is create-only (`PR-IAM-004` warns on change); a purely numeric
  `metadata.name` is refused (`PR-IAM-003`).

A context may tune defaults under an `iam:` block (`protected-roles`,
`known-roles`, `allow-admin-lockout`).
[Quick Start §8](docs/quickstart.md#8-managing-iam-users) ·
[`users.xml` migration](docs/migration.md#legacy-usersxml--onmsctl).

---

## SNMP configuration (`kind: SnmpConfig`)

Manage Horizon's SNMP configuration (`snmp-config.xml`: `defaults` + `profiles` +
`definitions`) declaratively, and read it back with `snmp export` / `snmp lookup`.
Targets `/api/v2/snmp-config`.

`SnmpConfig` is a **singleton** (`metadata.name` fixed to `default`): apply
reconciles by **whole-config replace** — pull the deployed config, compare ignoring
secret values, re-upload only when it differs.

```sh
onmsctl apply -f examples/snmp-config.yaml --dry-run --diff
onmsctl snmp export -O snmp-config.yaml          # deployed config → YAML (secrets as refs)
onmsctl snmp lookup 192.168.8.8 --show-secrets   # effective params OpenNMS would use
```

- **Secrets are write-only.** Communities and v3 passphrases are rejected inline;
  reference an external secret (same shape as IAM's `passwordRef`). They're
  excluded from the idempotency comparison, so a secret-only rotation isn't
  auto-detected — re-apply deliberately. `snmp export` emits placeholders, never
  cleartext.
- **Trap daemon** via optional `spec.trapd` (reconciled against
  `/api/v2/trapd/config` in the same apply; additive — omit it and the trap daemon
  is untouched). Requires a Horizon build with the Trapd REST API (NMS-19128, the
  `37.x`/`develop` line); against older servers the `trapd` half fails with a clear
  version message while the snmp-config half still applies.
- **Order:** `SnmpConfig` applies before `Requisition`, so a co-located directory
  configures SNMP before importing nodes. An SNMP change does not auto-rescan
  already-imported nodes — follow with `requisition import <fs> --rescan-existing`.

[Quick Start §9](docs/quickstart.md#9-snmp-configuration).

---

## Maintenance windows (`kind: Maintenance`)

Plan a maintenance window — for a period, stop OpenNMS polling, notifications, and
threshold (and optionally collection) evaluation on chosen devices. Maps to an
OpenNMS **scheduled outage** (`/rest/sched-outages`); named and multi-instance
(one document per window), reconciled like requisitions. No version gate — the API
is present in every supported Horizon/Meridian.

```yaml
apiVersion: maintenance.opennms.org/v1
kind: Maintenance
metadata: { name: weekend-patching }
spec:
  schedule:
    type: specific                  # specific | daily | weekly | monthly
    times:
      - { begins: "20-Jun-2026 22:00:00", ends: "21-Jun-2026 04:00:00" }
  devices:                          # selectors are a UNION, deduped to nodeIds
    interfaces: [192.168.8.8]       # an IP, or the single literal `match-any`
    nodes: [ { foreignSource: hq, foreignId: web01 } ]   # resolved to a nodeId at apply
    categories: [Routers]           # every node in ANY listed category
    locations: [Berlin]             # every node at ANY listed Minion location
    asset: { field: city, value: Berlin }
  suppress:
    polling: { packages: [production] }   # explicit packages required (no default)
    notifications: true                   # global (no package)
```

```sh
onmsctl apply -f examples/maintenance.yaml --dry-run --diff
onmsctl maintenance list
onmsctl maintenance status 192.168.8.8 12     # IP or nodeId → in a window now?
onmsctl maintenance delete weekend-patching    # full teardown
```

Apply writes the outage **definition** (a true `Created`/`Updated`/`Unchanged`),
then **attaches** it per daemon (`polling`→pollerd, `thresholds`→threshd,
`collection`→collectd per package; `notifications`→notifd global). Two things to
know:

- **Attachments are ensure-present.** The API can't read which daemons an outage is
  attached to, so onmsctl re-issues the idempotent attach every apply and **cannot
  detach** — removing a `suppress` entry does *not* detach it. To reduce
  suppression, `maintenance delete <name>` and re-apply.
- **Explicit packages; server timezone; foreignId nodes.** `polling`/`thresholds`/
  `collection` require an explicit `packages` list. Times are the **server's**
  timezone. Nodes are named by `{foreignSource, foreignId}` and resolved at apply
  (an un-imported node fails that window — which is why `Maintenance` applies after
  `Requisition`; prefer `interfaces`/`match-any` when nodes may be un-imported).
- **Dynamic selectors** (`categories`/`locations`/`asset`) are a **union, not an
  intersection**, re-resolved on every apply (a snapshot). A selector matching
  nothing warns; a window covering nothing fails.

---

## SNMP data collection (`kind: DataCollectionSource`)

Manage Horizon's SNMP data-collection config — which MIB objects, resource types,
and system definitions are collected — over the DB-backed
`/api/v2/datacollectionconf` surface. One `kind: DataCollectionSource` document per
`datacollection-group` (a "source"); onmsctl owns **only the sources you write** —
the stock vendor library is left untouched (additive prune, not a singleton).

```sh
onmsctl apply -f examples/datacollection-source.yaml
onmsctl datacollection list [--profiles]              # deployed sources / profiles
onmsctl datacollection export acme-router             # the group as xml|json
onmsctl datacollection delete acme-router
```

- **Whole-source replace** — a changed group tree is re-uploaded and the server
  upserts *and prunes* removed children; an unchanged source (normalized, order-
  insensitive) is a no-op.
- **`profiles` is the full truth (true reconcile)** — a name added is attached, a
  name dropped is detached. A new source must list ≥1 profile. An optional inline
  **`profileSpec`** creates/tunes that profile from zero.
- **Preflight version gate** — the endpoint is absent from released Horizon
  ≤ 37.0.0; apply probes it once and fails the whole apply early (before any write)
  on a server that lacks it.

`list`/`export` are Read; `apply`/`delete` are Write.

---

## Business services (`kind: BusinessService`)

Describe a Business Service Monitoring (BSM) hierarchy declaratively — a service,
its per-service **reduce function**, optional attributes, and four kinds of **edge**
(to child services, monitored IP services, applications, and raw alarm reduction
keys), each with a `weight` and optional per-edge **map function**. Maps to the v2
`/api/v2/business-services` surface; named and multi-instance (one document per
service). No version gate — the API is present in every supported Horizon/Meridian.

```yaml
apiVersion: bsm.opennms.org/v1
kind: BusinessService
metadata: { name: web-frontend }
spec:
  attributes: { owner: platform-team }
  reduceFunction:
    type: Threshold                 # HighestSeverity | Threshold | HighestSeverityAbove | ExponentialPropagation
    properties: { threshold: "0.75" }
  childServices:                    # → another BusinessService, by name
    - { name: database-tier, weight: 2, mapFunction: { type: Increase } }
  ipServices:                       # → a monitored service; node by label+location
    - node: { label: webhost01, location: Default }
      ipAddress: 10.0.0.10
      service: HTTP
  applications:                     # → an OpenNMS Application, by name
    - { name: Webservers }
  reductionKeys:                    # → a raw alarm reduction key (escape hatch)
    - reductionKey: "uei.opennms.org/threshold/highThresholdExceeded::{{nodeId}}:10.0.0.10:ifHCInOctets:90.0:3:75.0:Gi1/0/1"
      node: { label: webhost01 }    # resolves {{nodeId}} at apply
      mapFunction: { type: SetTo, properties: { status: Major } }
```

```sh
onmsctl apply -f examples/business-service.yaml --dry-run --diff
onmsctl business-service list                   # or: onmsctl bs list
onmsctl business-service get web-frontend
onmsctl business-service delete web-frontend     # explicit removal
```

References are **by name** (BSM's REST API is ID-centric; onmsctl resolves names →
ids at apply, so the same YAML is portable across instances). Things to know:

- **Whole-object reconcile.** A service is created then fully PUT (a destructive
  full-replace). Edges **omitted from a document are pruned**; an unchanged service
  (order-insensitive) is a no-op. Child references are resolved by a **two-pass**
  apply (create, then PUT with resolved ids), so two new services can reference each
  other in one file regardless of order; a child **cycle** fails at plan time.
- **Across-apply non-deletion.** A service present on the server but **absent** from
  your apply is **not** deleted — use `business-service delete <name>`.
- **Node references.** A `node` is `{label, location}` (the default location is
  `Default`) or `{foreignSource, foreignId}`. Labels are **not unique** in OpenNMS;
  an ambiguous label fails the apply and tells you to use the foreignSource/foreignId
  form.
- **Prefer `ipServices` over hand-written reduction keys.** An `ipServices` edge
  auto-covers the service's standard alarms (`nodeLostService`/`interfaceDown`/
  `nodeDown`), so you never type a node id. Reach for `reductionKeys` only for custom
  alarms (thresholds, custom UEIs, situations); the literal key must match a
  **resolved alarm** reduction key (copy it from a real alarm — not the event-def
  formula), and `{{nodeId}}` (plus `{{foreignSource}}`/`{{foreignId}}`/`{{nodeLabel}}`)
  is expanded from the edge's `node`.
- **Daemon reload.** Changes are inert until `bsmd` reloads; a mutating apply (and
  `delete`) issues exactly one `daemon/reload` automatically.

`list`/`get` are Read; `apply`/`delete` are Write.

---

## Conventions & tooling

### Editor integration

JSON Schemas (draft 2020-12) live under [`schemas/`](schemas/), one per kind. Add a
modeline to the top of your YAML for in-editor validation via
[`yaml-language-server`](https://github.com/redhat-developer/yaml-language-server):

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/event-source.schema.json
```

Swap the filename for `requisition` / `iam-user` / `snmp-config` / `maintenance` /
`datacollection`. Pin a release tag for stability, or reference a local clone
(`./schemas/<name>.schema.json`). Regenerate with `make schema` (CI fails if a
committed artifact lags). The requisition schema annotates list fields with
`x-onmsctl-list-kind: ordered|set` so diff tooling distinguishes ordered sequences
(`detectors`, `policies`) from sets (`categories`, `services`).

### Output formats

Every list/get accepts `-o table` (default), `-o yaml`, `-o json`.

### Verb aliases & read-only contexts

Short aliases: `event-source`→`evtsrc`, `event`→`evt`, `requisition`→`req`,
`config`→`cfg`, `maintenance`→`maint`, `datacollection`→`dc` (both forms appear in
`--help`).

Mark a context `read-only: true` (or pass `--read-only` / set `$ONMSCTL_READ_ONLY`)
to refuse every Write verb locally — exit code `12` — before any HTTP call.
Precedence is **flag > env > context > default false**; the flag is one-way.
`--dry-run apply` classifies as a Read, so it runs in read-only contexts.

### Exit codes

Stable and safe for scripting:

| Code | Meaning | | Code | Meaning |
|---|---|---|---|---|
| 0 | success | | 9 | unsupported auth scheme |
| 1 | HTTP non-success / partial-failure batch | | 10 | `--wait` timed out |
| 2 | misuse / config error | | 11 | `--wait` saw the async op fail |
| 4 | DNS failure | | 12 | write refused by read-only context |
| 5 | connection refused | | 13 | apply refused: admin lockout (IAM-001) |
| 6 | timeout | | 14 | apply refused: self lockout (IAM-002) |
| 7 | TLS handshake failed | | 15 | apply refused: `whoami` unavailable |
| 8 | redirect loop | | | |

### Shell completions

```sh
onmsctl completion bash > /etc/bash_completion.d/onmsctl
onmsctl completion fish > ~/.config/fish/completions/onmsctl.fish
onmsctl completion zsh  > "$(brew --prefix)/share/zsh/site-functions/_onmsctl"   # adjust target to your setup
```

For Oh My Zsh, write to `~/.oh-my-zsh/custom/completions/_onmsctl` and add
`fpath=("$ZSH_CUSTOM/completions" $fpath)` to `~/.zshrc` above the
`source $ZSH/oh-my-zsh.sh` line. The script targets the literal name `onmsctl` —
`sed -e 's/onmsctl/<name>/g'` if you've repackaged it.

### TLS

`server.insecure-skip-tls-verify: true` (or `--insecure-tls`) disables certificate
verification and emits a per-request stderr warning. Keep it off in production.

---

## Server compatibility

| Server | Status |
|---|---|
| OpenNMS Horizon **35+** | Primary target (EventConf REST reproducible on 35.0.5 / 36.0.0). |

Some capabilities need newer builds: the SNMP **Trapd** block (NMS-19128, `37.x`/
`develop`) and **data collection** (absent from released Horizon ≤ 37.0.0) — both
gate cleanly with a clear version message on older servers.

**Known eventconf quirks** (Horizon 35.0.5 / 36.0.0, tracked upstream; onmsctl works
around the load-bearing ones client-side):

- `event-source list` can print empty despite existing sources
  ([NMS-19810](https://opennms.atlassian.net/browse/NMS-19810)) — use
  `event-source names-and-ids`; `apply --diff` may show a whole document as "added"
  rather than a true delta, though the upload still succeeds.
- `POST /eventconf/upload` requires the multipart field name to be literally
  `upload` ([NMS-19813](https://opennms.atlassian.net/browse/NMS-19813)) and derives
  the source name by stripping only the final extension — onmsctl handles both, and
  uploads `{metadata.name}.xml` so the stored name equals `metadata.name`.

---

## License

Apache-2.0. Third-party crate licenses are inventoried in `THIRD-PARTY-LICENSES.md`
(regenerated by `make licenses`).

## Contributing

See `CONTRIBUTING.md`. Implementations work from the OpenAPI document and black-box
observation of a Horizon instance — never from Horizon's server source — so the
result stays an Apache-2.0 clean-room.
