# onmsctl

A `kubectl`-style command-line interface for [OpenNMS Horizon][horizon].
A single declarative entrypoint — top-level `onmsctl apply -f` — peeks
each YAML document's `kind` and routes it to the right handler, so users,
event sources, and provisioning requisitions all reconcile through one
command. XML→YAML migrators bring legacy eventconf and `provision.pl`-shape
requisition files into that loop; reads, explicit deletes, and `convert`
stay imperative.

[horizon]: https://www.opennms.com/horizon/

> **Pre-stability notice.** Releases on `v0.x.y` may break CLI flags, the
> configuration schema, and the `EventSource` YAML schema between minor
> versions. Surfaces stabilize at `v1.0.0`.

> **New to onmsctl?** Start with the [Quick Start guide](docs/quickstart.md) —
> install, configure a context, and run your first `apply` in a few minutes.

---

## Install

Pre-compiled binaries are published as GitHub Releases for every `v*.*.*`
tag. Each release ships per-target binaries, per-binary SHA256 checksums,
an aggregate `SHA256SUMS` file, and Sigstore (cosign) keyless signatures
+ certificates for every asset.

| Target                          | Asset suffix                       |
|---------------------------------|------------------------------------|
| Linux x86_64                    | `x86_64-unknown-linux-gnu`         |
| Linux aarch64                   | `aarch64-unknown-linux-gnu`        |
| macOS x86_64 (Intel)            | `x86_64-apple-darwin`              |
| macOS aarch64 (Apple Silicon)   | `aarch64-apple-darwin`             |

Windows is not yet in the release matrix; Windows users build from
source (below).

```sh
VERSION=v0.1.0
TARGET=x86_64-apple-darwin  # or one of the rows above

curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.sha256

shasum -a 256 -c onmsctl-${VERSION}-${TARGET}.sha256

chmod +x onmsctl-${VERSION}-${TARGET}
sudo mv onmsctl-${VERSION}-${TARGET} /usr/local/bin/onmsctl

onmsctl version
```

**Verify the cosign signature (recommended).** Every release asset is
signed via Sigstore's keyless OIDC flow. Verifying ties the binary to a
specific GitHub Actions workflow run on this repository, with no
long-lived key:

```sh
cosign verify-blob \
  --certificate-identity-regexp "^https://github.com/no42-org/onmsctl/.github/workflows/release.yml@" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate onmsctl-${VERSION}-${TARGET}.pem \
  --signature  onmsctl-${VERSION}-${TARGET}.sig \
  onmsctl-${VERSION}-${TARGET}
```

**macOS Gatekeeper.** Binaries are Sigstore-signed but not Apple-notarized.
On first run macOS may quarantine the file:

```sh
xattr -d com.apple.quarantine /usr/local/bin/onmsctl
```

or approve once via System Settings → Privacy & Security.

---

## Build from source

For development, unreleased commits, or platforms outside the release
matrix. Requires the toolchain pinned in `rust-toolchain.toml` (currently
Rust 1.95):

```sh
git clone https://github.com/no42-org/onmsctl
cd onmsctl
make build              # debug build → target/debug/onmsctl
cargo build --release   # → target/release/onmsctl
cargo install --path crates/onmsctl   # → ~/.cargo/bin
```

Verify with `onmsctl version`.

### About this binary

`onmsctl` is one binary that statically links every capability crate
in the workspace. Run `onmsctl version` to see the binary version
alongside each linked capability:

```
$ onmsctl version
onmsctl 0.0.1
capabilities:
  - eventconf 0.0.1
  - provisioning 0.0.1
  - iam 0.0.1
```

Each capability owns its own subcommand tree (`event-source` / `event`,
`requisition`, `iam`) and ships its own JSON Schemas, validators, and
HTTP client wrappers. New capabilities are added as workspace
members; this list grows verbatim from `onmsctl_<cap>::{CAPABILITY_NAME,
VERSION}`.

---

## Configure a context

kubectl pattern: one config file, one or more named contexts, one
currently-active context.

| OS      | Default path |
|---------|--------------|
| Linux   | `$XDG_CONFIG_HOME/onmsctl/config.yaml` (typically `~/.config/onmsctl/config.yaml`) |
| macOS   | `~/Library/Application Support/org.no42-org.onmsctl/config.yaml` |
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
        password: admin
```

> **Don't commit inline secrets.** Use one of the credential references
> below.

### Credentials

`auth.basic` accepts exactly one of `password` / `password-file` /
`keyring`. `auth.bearer` accepts the same shapes as `token` /
`token-file` / `keyring`.

| Field | Notes |
|---|---|
| `password` / `token` | Inline plain-text. Convenient, leaks if the config leaks. |
| `password-file` / `token-file` | Mode `0600` recommended; trailing newlines stripped. |
| `keyring` | OS keyring. macOS Keychain and Windows Credential Manager work out of the box. On Linux the default build links only the kernel keyutils backend; for GNOME Keyring / KWallet support rebuild with `--features keyring/sync-secret-service` (adds a `libdbus-1` build dependency). |

Resolution at request time:

```
env ($ONMS_PASSWORD / $ONMS_TOKEN)  >  keyring  >  file  >  inline
```

### Switching contexts

```sh
onmsctl config view                  # current config (secrets redacted)
onmsctl config use-context staging   # atomic rewrite of current-context
```

`config view` redacts inline `password` / `token` strings; file and
keyring references remain visible (they are pointers, not secrets).
`config use-context` resolves symlinks before writing so a symlinked
`config.yaml` writes through to the upstream file.

### Override precedence

```
flags (--url, --user, --context)  >  env (ONMS_URL, ONMS_USER, ONMSCTL_CONTEXT)  >  active context  >  built-in default
```

---

## Declarative apply (`onmsctl apply -f`)

`onmsctl apply -f <file|dir|glob>` is the single declarative mutation
entrypoint. It peeks each YAML document's `kind` and routes it to the
registered handler — no per-capability apply verb. Three kinds are
recognized:

| `kind` | `apiVersion` | Reconciles |
|---|---|---|
| `User` | `onmsctl.no42.org/v1alpha1` | Horizon users + roles |
| `EventSource` | `eventconf.opennms.org/v1` | event configuration sources |
| `Requisition` | `provisioning.opennms.org/v1` | provisioning requisitions |

A single file may hold many documents (`---`-separated), and a directory
can mix all three kinds.

**Plan → gate → execute.** Every document is planned first. If *any*
document fails to plan — an unknown `kind`, a duplicate `metadata.name`,
a parse error — the whole apply **aborts before any mutation**. Once the
plan gate passes, documents execute in a static precedence order:

```
User (100)  →  EventSource (200)  →  Requisition (300)
```

**Input.** `-f` accepts a single file, a directory of `*.yaml` / `*.yml`
(non-recursive by default), or a glob (quote it so the shell doesn't
pre-expand). `apply` has no short alias.

```sh
onmsctl apply -f users.yaml                       # single file
onmsctl apply -f ./desired-state/                 # directory (mixed kinds)
onmsctl apply -f ./desired-state/ -R              # recurse into subdirs
onmsctl apply -f 'sources/cisco-*.yaml'           # glob
```

**Flags.**

| Flag | Behavior |
|---|---|
| `--dry-run` | Plan only; issues zero mutating HTTP. Classifies as a Read, so `--read-only` contexts may run it for review. |
| `--diff` | Render each kind-bucket's diff to stderr. |
| `--continue-on-error` (alias `--keep-going`) | Keep applying after a failing document. Default is stop-on-error — halt after the first failing document and report the rest as not-attempted. |
| `-R` / `--recursive` | Recurse into subdirectories. Off by default; ignored with a stderr note when `-f` is not a directory. |

**Exit codes.** `0` when every document applied or was unchanged; `1`
when any document fails (including a plan-gate failure or an unknown
`kind`); `2` on usage error (bad flags, empty / no input).

### Removed imperative verbs → `onmsctl apply -f`

The imperative mutators below no longer exist. Declare the desired state
in YAML and `apply` it instead:

| Removed verb(s) | Replacement |
|---|---|
| `event-source apply`, `event-source create`, `event-source enable`, `event-source disable` | Declare the source, its events, and enabled-state in a `kind: EventSource` document, then `onmsctl apply -f`. (`event-source upload` / `event-source download` still round-trip raw XML.) |
| `event add`, `event update`, `event delete`, `event enable`, `event disable` | Edit `spec.events[...]` in the owning `kind: EventSource` document, then `onmsctl apply -f`. (`event list` remains for inspection.) |
| `requisition apply` | `onmsctl apply -f` (kind `Requisition`). |
| `requisition node\|interface\|service\|category add\|set\|remove` | Edit `spec.nodes[...]` in the requisition YAML, then `onmsctl apply -f`. The matching `… list` / `get` sub-resource verbs remain for inspection. |
| `iam apply`, `iam user create`, `iam user update`, `iam user role add`, `iam user role remove` | Declare a `kind: User` document (scalar fields + `roles` set + `passwordRef`), then `onmsctl apply -f`. `iam user set-password`, `iam user delete`, and the read verbs remain. |

---

## GitOps for OpenNMS event configuration

Keep event configuration in git as YAML; push to Horizon declaratively.
Two commands carry the loop — `event-source convert` brings existing XML in,
`onmsctl apply -f` ships edits out.

### Step 1 — Convert existing XML → YAML

Pure local file transform; no Horizon contact required.

```sh
onmsctl event-source convert /opt/opennms/etc/events/cisco.foo.events.xml
onmsctl event-source convert --output-dir yaml/ /opt/opennms/etc/events/*.events.xml
```

Rule violations emit stable, file:line-anchored findings on stderr (see
[`event-source convert` reference](#source-convert) for the finding-code
catalog, exit codes, flags, and the unmodeled-element policy).

### Step 2 — Apply YAML to Horizon

```sh
onmsctl apply -f cisco.foo.yaml --diff
```

Fetches the server's current state, prints a UEI-bucketed diff to
stderr, uploads only when changes exist. Add `--dry-run` to simulate.

`onmsctl apply -f` accepts a single file, a directory of YAML files, or
a glob pattern (quote it so the shell doesn't pre-expand):

```sh
onmsctl apply -f sources/                  # directory
onmsctl apply -f 'sources/cisco-*.yaml'    # glob
```

Every document is planned before any write (the plan gate aborts the
whole apply if any document fails to plan), then applied in precedence
order. Across multiple documents, stop-on-error is the default — pass
`--continue-on-error` to keep going and collect failures, with a
non-zero exit if any document failed.

`metadata.name` becomes the server's stored source name verbatim;
Horizon derives `vendor` server-side as the prefix before the first `.`
(so `metadata.name: cisco.foo` → source name `cisco.foo`, vendor `cisco`).

The `examples/` directory ships fixtures: `minimal.yaml`, `full.yaml`
(every nested type), `severities.yaml`, `disabled.yaml`. Full schema
detail in the [EventSource YAML reference](#eventsource-yaml-schema).

### Step 3 — Iterate

Edit YAML in git, push through review, run `onmsctl apply -f` again. The
diff display flags changes the upload would make. `--dry-run` is safe
for any branch; `apply` itself is idempotent (Horizon's upsert
path replaces events under an existing basename).

### Export deployed sources back to YAML

The reverse of `apply`: snapshot server-side event sources as
`kind: EventSource` YAML for git-managed sync (the eventconf twin of
`requisition export`).

```sh
# One source (by numeric id or exact name) to stdout
onmsctl event-source export cisco.foo

# One source to a directory (single <name>.yaml file)
onmsctl event-source export cisco.foo --out ./sources/

# Every source, one <name>.yaml file each
onmsctl event-source export --out ./sources/
```

Both `event-source export` and `event-source download` accept a selector
that is either a numeric **id** or an exact source **name**, and `--out`
works with or without a selector. Because the server only emits XML, export
runs each source through the `convert` migrator — the same `EC###` findings
apply (to stderr). Bulk export is **continue-on-error**: sources that convert
are written (warnings included), blocking ones are skipped, a source that
fails to download is reported and counted, and the process exits with the
highest severity observed. `--out` is all-or-nothing — every target filename
is validated before any file is written.

### Editor integration

JSON Schemas (draft 2020-12) live under [`schemas/`](schemas/), one per
capability YAML kind. One line at the top of your YAML enables in-editor
validation via [`yaml-language-server`](https://github.com/redhat-developer/yaml-language-server):

```yaml
# For kind: EventSource
# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/event-source.schema.json

# For kind: Requisition
# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/requisition.schema.json
```

Pin to a release tag for stability or reference a clone with
`./schemas/<name>.schema.json`. Regenerate with `make schema`; CI
fails if any committed artifact lags.

**Project-specific extension** — the requisition schema annotates list
fields with `x-onmsctl-list-kind: ordered|set` so downstream diff
tooling distinguishes ordered sequences (`detectors`, `policies` —
order is semantically meaningful in provisiond) from sets
(`categories`, `services` — order is ignored). Editors that don't
understand the extension ignore it harmlessly.

---

## GitOps for OpenNMS provisioning

Manage Horizon requisitions (the `provision.pl`-shape data) declaratively
from git. The loop is: `requisition convert` brings existing XML in,
`onmsctl apply -f` ships edits out, `requisition status` / `import` cover
the lifecycle.

### Step 1 — Convert existing XML → YAML

Pure local file transform; no Horizon contact required.

```sh
# Single pair: requisition XML + matching foreign-source XML
onmsctl requisition convert \
    --from /opt/opennms/etc/imports/ \
    --foreign-sources-dir /opt/opennms/etc/foreign-sources/ \
    --out yaml/

# Stream to stdout (one document per requisition, separated by `---`)
onmsctl requisition convert --from /opt/opennms/etc/imports/
```

Rule violations emit `PR###`-coded findings on stderr. Run
`onmsctl requisition convert --explain PR001` to see the rationale
for any code. PR001 / PR002 / PR003 are Warnings (exit 1), PR004 is
Info (exit 0), and parse failures exit 2.

XML elements the YAML model doesn't represent are **preserved** under
`metadata.x-onmsctl-unmodeled` rather than silently dropped, so a
`convert → apply → export → re-apply` round-trip keeps server-side
fields onmsctl doesn't model yet. The annotation is stripped before
the three-level diff and before the body is sent to Horizon, so it
never changes apply outcome; `apply --diff` collapses it to a one-line
`metadata.x-onmsctl-unmodeled: N entries` summary.

### Step 2 — Apply YAML to Horizon

```sh
# Single file — preview without writing. --diff goes to stderr,
# structured outcome to stdout. Safe against any context.
onmsctl apply -f acme-prod.yaml --dry-run --diff

# Single file — real apply.
onmsctl apply -f acme-prod.yaml

# Directory mode — every *.yaml / *.yml under the path is planned
# first. The plan gate runs a cross-file collision check (duplicate
# metadata.name aborts before any writes; duplicate foreignId warns
# and continues). Stop-on-error is the default — halt after the first
# failing document; pass --continue-on-error to keep going.
onmsctl apply -f ./requisitions/ --dry-run
onmsctl apply -f ./requisitions/
```

The apply path computes a three-level diff (canonical-bytes / per-node /
per-leaf), auto-decides `rescanExisting` from the scan-relevance of what
changed, writes the foreign-source + requisition, and triggers the import —
fire-and-forget. The declarative path always uses the auto `rescanExisting`
decision; there is no override flag.

`--diff` renders each kind-bucket's diff to stderr — for directory previews
use `--dry-run -o yaml` to see the structured outcomes per document. Import
completion is asynchronous server-side: to block on it, run the lifecycle
verb `onmsctl requisition import <fs> --wait --timeout 5m`, or poll
`onmsctl requisition status <fs>`.

The plan phase parses every document first — running the cross-file
collision check and a per-document diff (GET only) — before any write.
Each document then produces one `ApplyOutcome` row, rendered through the
global `-o table|yaml|json`. A `--dry-run` shows the predicted action
without mutating:

```text
kind         name       action  status     message
Requisition  acme-prod  create  Skipped    dry-run: would create
Requisition  site-b     none    Unchanged  in sync
Requisition  lab-east   update  Skipped    dry-run: would update
```

A parse error, an unknown `kind`, or a hard `metadata.name` collision
fails at the plan gate before any HTTP write is issued: the process exits
`1` with a message naming the offending document, and nothing is mutated.

### Step 3 — Iterate

Edit YAML in git, push through review, run `onmsctl apply -f`
again. `--dry-run` is safe for any branch; `--diff` prints the diff to
stderr so `-o json`/`-o yaml` consumers on stdout stay clean.

### Pinned vs portable YAML

The composite `kind: Requisition` document carries `spec.foreignSource`
optionally. Two operator styles:

- **Pinned style** — include `spec.foreignSource` with detectors and
  policies. The YAML is the source of truth for both the requisition
  and its foreign-source. `apply` creates / updates the custom
  foreign-source alongside the requisition.
- **Portable style** — omit `spec.foreignSource` entirely. On apply,
  Horizon's default foreign-source is inherited (no custom detectors,
  no custom policies). Useful for stamping out cookie-cutter
  requisitions that all share the site-wide default scan settings. If
  the server previously had a custom foreign-source for this name,
  `apply` will **delete it** and the `--diff` output enumerates the
  displaced detectors and policies so the operator sees the blast
  radius.

The [`examples/requisition-acme-prod.yaml`](examples/requisition-acme-prod.yaml)
fixture demonstrates the pinned style with every modeled field.

### Migration map: `provision.pl <verb>` → `onmsctl`

| `provision.pl` | `onmsctl` |
|---|---|
| `provision.pl requisition add <fs>` | `onmsctl apply -f <fs>.yaml` (a `kind: Requisition` document with an empty `nodes: []` payload) |
| `provision.pl requisition remove <fs>` | `onmsctl requisition delete <fs> --yes` (issues both `DELETE /rest/requisitions/<fs>` AND `DELETE /rest/requisitions/deployed/<fs>` in one call; idempotent on both — 404 on either snapshot is treated as success. **`--yes` is required in non-TTY contexts (CI / scripting); TTY contexts prompt interactively.** Remove the local YAML separately) |
| `provision.pl requisition import <fs>` | `onmsctl requisition import <fs>` (PUT-only, no re-POST; add `--wait` to block until completion) |
| `provision.pl requisition list` | `onmsctl requisition list` (wraps `GET /rest/requisitionNames`; respects `-o` table / json / yaml) |
| `provision.pl node add <fs> <foreign-id> <node-label>` | Edit `spec.nodes[...]` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition node list / get` remain for inspection. |
| `provision.pl interface add <fs> <foreign-id> <ip>` | Edit the node's `interfaces` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition interface list / get` remain for inspection. |
| `provision.pl service add <fs> <foreign-id> <ip> <svc>` | Edit the interface's `services` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition service list` remains for inspection. |
| `provision.pl category add <fs> <foreign-id> <cat>` | Edit the node's `categories` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition category list` remains for inspection. |
| `provision.pl asset add <fs> <foreign-id> <name> <value>` | Edit the node's `spec.nodes[].assets` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. Post-import, takes-effect-immediately escape hatch: `onmsctl requisition asset set <db-id> <field> <value>` (sibling reads: `asset list / get`). **Misfit:** keyed by integer database node ID, not foreign-id — resolve via `curl /opennms/rest/nodes?foreignId=<fid>` before running. |

The migration philosophy reverses `provision.pl`'s shell-automation
model. Where `provision.pl` ran one mutation per invocation,
`onmsctl apply` ships the desired state and lets the
three-level diff figure out what to mutate.

### Migrating off `provision.pl` shell automation

Recommended once-per-site recipe (per design D11):

1. **Convert** existing XML to YAML with `onmsctl requisition convert
   --from /opt/opennms/etc/imports/ --foreign-sources-dir
   /opt/opennms/etc/foreign-sources/ --out repo/yaml/`. Review the
   stderr findings; resolve PR001 / PR002 by editing the source XML
   (rare) or accepting the documented data-loss (most common — see
   the per-code `--explain` text).
2. **Commit** the YAML directory to git as the new source of truth.
3. **Rewrite** the existing `provision.pl` shell scripts as `onmsctl
   apply -f <fs>.yaml` invocations. The YAML carries
   desired state; the legacy "step-by-step mutation" pattern
   collapses to one apply per requisition.
4. **Schedule** the apply via CI / cron. `--dry-run --diff` is the
   review gate; the actual apply runs only after review.

For ongoing sync after the initial migration — operators that edit
requisitions in Horizon's UI and want to pull the changes back into
git — use `requisition export`:

```sh
# Export every requisition on the server, per-file into a directory.
onmsctl requisition export --out repo/yaml/

# Export a single requisition to stdout.
onmsctl requisition export acme-prod

# Inline Horizon's default-FS into the YAML when the requisition has
# no custom one. Adds a snapshot-timestamp comment so the operator
# sees what defaults were in effect at export time.
onmsctl requisition export --include-defaults --out repo/yaml/
```

Without `--include-defaults`, the exported YAML omits
`spec.foreignSource` when the server has no custom FS — i.e. produces
the portable style described above. With `--include-defaults`, the
default-FS is inlined alongside a snapshot comment; the inlined block
is a point-in-time copy that does NOT stay in sync with Horizon's
default after export.

### Editor integration

See the [editor integration](#editor-integration) section above for
the `yaml-language-server` directive for `kind: Requisition` — the
schema lives at `schemas/requisition.schema.json` and ships with the
same `x-onmsctl-list-kind` annotations the diff engine uses to
distinguish ordered lists (detectors / policies) from sets
(categories / services).

### Breaking changes since v0.1.0

#### `onmsctl requisition delete <fs>` now requires `--yes` confirmation

The verb purges both pending and deployed snapshots in a single
call, which has a wider blast radius than any other Write verb in
the CLI. Starting in v0.1.1 it refuses to run without explicit
operator acknowledgement:

- **Interactive (TTY) shells:** the verb shows the requisition name
  + node count + last-import timestamp (ISO-8601 UTC) and prompts
  for `yes` or `y` (case-insensitive). Any other input — including
  `no`, empty line, or Ctrl-D / EOF — aborts with a **non-zero**
  exit so calling scripts can distinguish cancellation from success.
- **Non-interactive contexts (CI, scripted pipelines, redirected
  stderr):** the verb refuses with a clear error pointing at
  `--yes`. There is no "auto-confirm because non-interactive" path.
  Both stdin AND stderr must be TTYs for the interactive prompt
  to fire.

**CI fix recipe:** add `--yes` (or `-y`) to existing invocations:

```sh
# Before:
onmsctl requisition delete acme-prod

# After:
onmsctl requisition delete acme-prod --yes
```

If the requisition already doesn't exist on the server (both
snapshots return 404), the verb is a no-op and skips the prompt.

---

## IAM (users + roles)

Manage Horizon users and their roles — declaratively via `onmsctl apply
-f` (`kind: User`, the GitOps loop) and imperatively via the read /
rotate / delete `iam user ...` verbs. Targets the `users` REST surface
on Horizon 35+.

### `whoami`

```sh
onmsctl iam whoami        # prints the calling user (GET /users/whoami)
```

`whoami` is also the safety check the apply path uses before any
self-affecting change (see *Lockout protection* below).

### Declarative apply (`onmsctl apply -f`)

```sh
# Preview — plan only, no writes. --diff renders the per-user plan and
# the summary to stderr. Safe in any context, including --read-only
# (a dry-run apply classifies as a Read verb).
onmsctl apply -f examples/iam-user.yaml --dry-run --diff

# Real apply. Continue-on-error per user with --continue-on-error
# (alias --keep-going); stop on the first per-user failure by default.
onmsctl apply -f ./users/ --continue-on-error

# Directory mode — every *.yaml / *.yml under each -f path. A duplicate
# metadata.name across documents aborts before any write (PR-IAM-002).
onmsctl apply -f ./users/ --dry-run
```

`kind: User` documents flow through the same `onmsctl apply -f` plan →
gate → execute path as every other kind. Apply plans every user first
(per-user GET + one `GET /users` lockout snapshot), then executes in the
order creates → updates → role deltas. Each user produces one
`ApplyOutcome` row, rendered through the global `-o table|yaml|json`, with
any `PR-IAM-*` warnings carried in the row's message / details. Roles
reconcile as a **set**: declaring `roles: [B, C]` against a server that
holds `[A, B]` grants `C` and revokes `A`, leaving `B`. An omitted
scalar field never clears the server value (merge semantics, not
replace). A role outside the built-in known set warns (`PR-IAM-006`)
but still applies.

The `dutySchedule` field is **create-only** (§D11.5): settable on the
initial create, but a change to it on an existing user warns
(`PR-IAM-004`) instead of mutating — other fields in the same plan
still apply.

A purely numeric `metadata.name` is refused at parse time (`PR-IAM-003`,
no override): the upstream `{userCriteria}` path segment is ambiguous
between a username and a database ID. Rename such accounts server-side
before declaring them in YAML.

### `passwordRef` — passwords are create-only and never inline

A literal `password:` key in user YAML is **rejected at parse time**
(`PR-IAM-001`). Reference an external secret instead, with exactly one
source:

```yaml
spec:
  passwordRef:
    fromFile: /run/secrets/alice.pw    # mode-checked, ≤4 KiB, no symlinks
  # passwordRef: { fromEnv: ALICE_PW }
  # passwordRef: { fromKeyring: { service: onmsctl, account: alice } }
```

A Create with no `passwordRef` is **refused** (`PR-IAM-005`) — a new
user must carry a password source; the server rejects a passwordless
create. `passwordRef` is honored on **Create only** — `apply` never
rotates an existing user's password (it can't read the current one to
diff). Rotate explicitly with `iam user set-password` (below). Horizon
hashes the plaintext server-side (`?hashPassword=true`); onmsctl never
sends a
precomputed hash, and the resolved secret is held in a `zeroize`-wrapped
string that redacts in every diff and debug path.

### Lockout protection

Apply refuses any plan that would lock administration out of the server:

- **`IAM-001` (admin lockout, exit 13):** emptying a protected role's
  holder set — by default `ROLE_ADMIN` — when it was non-empty before.
  Override by setting `iam.allow-admin-lockout: true` in the context
  config — a persisted, reviewable confirmation rather than an ad-hoc
  CLI flag.
- **`IAM-002` (self lockout, exit 14):** removing a protected role from,
  or deleting, the **calling** user (resolved via `whoami`). **No
  override** — re-run as a different admin.

If a self-affecting action is planned but `whoami` is unavailable (token
auth without an associated user, or a non-2xx response), apply refuses
with `IamWhoamiUnavailable` (exit 15) rather than skip the check.
`--continue-on-error` controls per-user execution failures only; it never
bypasses a plan-phase refusal (`IAM-001/002`, `PR-IAM-002`).

**Per-context overrides.** A context may tune the defaults under an
`iam:` block:

```yaml
contexts:
  - name: prod
    server:
      url: https://horizon.prod.example/opennms
    iam:
      protected-roles: [ROLE_ADMIN, ROLE_REST]   # default: [ROLE_ADMIN]
      known-roles: [ROLE_ADMIN, ROLE_USER, ...]  # replaces the built-in set
      allow-admin-lockout: true                  # default: false; persisted IAM-001 override
```

`protected-roles` replaces the `[ROLE_ADMIN]` default (an explicit
empty list disables the admin-lockout check); `known-roles` replaces
the built-in `PR-IAM-006` validation set; `allow-admin-lockout: true`
is the persisted, reviewable override for the `IAM-001` admin-lockout
refusal. All apply to `onmsctl apply -f` of `kind: User` documents.

### Imperative quick-reference

User creation, field edits, and role grants/revokes are declarative —
declare a `kind: User` document and `onmsctl apply -f`. The surviving
imperative verbs cover reads, password rotation, and deletion:

```sh
# read
onmsctl iam user list                    # -o table | -o yaml | -o json
onmsctl iam user get alice
onmsctl iam user export                   # all users as declarative YAML
onmsctl iam user export --name alice      # one user to stdout

# delete
onmsctl iam user delete alice --yes       # --yes skips the TTY prompt

# rotate a password (one source; mutually exclusive)
printf %s "$NEW_PW" | onmsctl iam user set-password alice --password-stdin
onmsctl iam user set-password alice --from-file /run/secrets/alice.pw
onmsctl iam user set-password alice --from-keyring onmsctl/alice
```

`set-password` takes exactly one password source: `--from-file`,
`--from-env`, `--from-keyring <service>/<account>`, or `--password-stdin`
(piped input only — it refuses on a TTY so the secret can't echo).
`delete` prompts for confirmation on a TTY and refuses without `--yes`
in non-interactive contexts. `delete --yes` is idempotent: a 404 reports
"nothing to do".

### Migration map: legacy `users.xml` → `onmsctl`

| Pre-onmsctl | `onmsctl` |
|---|---|
| Hand-edit `$OPENNMS_HOME/etc/users.xml`, reload | `onmsctl apply -f users/` (one `kind: User` document per user; apply reconciles scalar fields + role set against the live server) |
| Add a user via the web UI | Add a `kind: User` document and `onmsctl apply -f` |
| Change a user's roles in the UI | Edit the document's `roles` set and `onmsctl apply -f` (roles reconcile as a set) |
| Rotate a password in the UI | `onmsctl iam user set-password <name> --password-stdin` |
| Remove a `<user>` element + reload | `onmsctl iam user delete <name> --yes` (idempotent; 404 → no-op) |

The example [`examples/iam-user.yaml`](examples/iam-user.yaml) covers
every modeled field with inline notes.

---

## Aliases and read-only contexts

### Top-level verb aliases

Every capability registers a short alias so the most common verbs
stay terse:

| Long form | Alias | Notes |
|---|---|---|
| `onmsctl event-source ...` | `onmsctl evtsrc ...` | eventconf source verbs |
| `onmsctl event ...` | `onmsctl evt ...` | eventconf event verbs |
| `onmsctl requisition ...` | `onmsctl req ...` | provisioning verbs |
| `onmsctl config ...` | `onmsctl cfg ...` | local config management |

Both forms appear in `--help` output so the alias is discoverable.

### Read-only contexts

Mark a context `read-only: true` to refuse every write verb locally
before any HTTP call. This is defense-in-depth on top of the server's
role checks — useful for "look but don't touch" credentials.

```yaml
contexts:
  - name: prod-readonly
    server:
      url: https://horizon.prod.example/opennms
    auth:
      basic:
        username: viewer
        keyring: prod-readonly
    read-only: true
```

Verbs classified `WriteCmd` at compile time — `onmsctl apply` (a Write
unless `--dry-run`), `event-source delete` / `upload`, `requisition delete` /
`import`, `requisition asset set`, `iam user delete` / `set-password` —
refuse with exit code 12 against a read-only context. Reads pass
through. The `--read-only` flag and `$ONMSCTL_READ_ONLY` env var
force the flag on regardless of context — precedence is **flag > env >
context > default false**, and the flag is one-way (no `--no-read-only`
escape hatch — context can never un-set it).

---

## Imperative operations

For ad-hoc work outside the GitOps loop. Source and event *mutation* is
now declarative — declare a `kind: EventSource` document and `onmsctl
apply -f` (see [Declarative apply](#declarative-apply-onmsctl-apply--f)).
The verbs below are reads, raw-XML round-trips, and the explicit source
delete:

```sh
# sources
onmsctl event-source list                  # -o table | -o yaml | -o json
onmsctl event-source get 42
onmsctl event-source delete 42 43
onmsctl event-source names                 # name-only listing
onmsctl event-source names-and-ids

# events (read-only; refs are <source-id>/<event-id>)
onmsctl event list --source 42
onmsctl event list --uei "uei.opennms.org/vendor/cisco/.*"   # cross-source
onmsctl event list --vendor cisco

# raw XML round-trip
onmsctl event-source upload cisco.foo.events.xml acme.widget.events.xml
onmsctl event-source download 42 -O cisco.foo.events.xml
onmsctl event-source download 42 --format yaml -O cisco.foo.yaml   # convert inline
```

`event-source download → edit → apply` may drop server-only fields not
modeled locally — keep the XML alongside for full fidelity.

---

## Server compatibility

| Server                  | Status |
|-------------------------|--------|
| OpenNMS Horizon **35+** | Primary target. EventConf REST endpoints are reproducible on 35.0.5 and 36.0.0. |

### Known server-side issues

Five `/eventconf/*` quirks reproducible on Horizon 35.0.5 and 36.0.0;
all tracked upstream. `onmsctl` works around NMS-19813 and the
filename-stripper quirk client-side.

| # | Endpoint | Symptom | Upstream |
|---|---|---|---|
| 1 | `GET /eventconf/filter/sources` | returns empty `eventConfSourceList` despite non-zero `totalRecords` | [NMS-19810](https://opennms.atlassian.net/browse/NMS-19810) |
| 2 | `GET /eventconf/filter/{id}/events` | HTTP 500 NPE when `offset` is omitted | [NMS-19811](https://opennms.atlassian.net/browse/NMS-19811) |
| 3 | `/eventconf/filter/*` | paging-parameter requirements differ per endpoint, undocumented | [NMS-19812](https://opennms.atlassian.net/browse/NMS-19812) |
| 4 | `POST /eventconf/upload` | HTTP 400 (empty body) unless the multipart field name is literally `upload` — CXF `@Multipart("upload")` qualifier on the JAX-RS interface | [NMS-19813](https://opennms.atlassian.net/browse/NMS-19813) |
| 5 | `POST /eventconf/upload` | `EventConfRestService.stripPathAndExtension` derives the source name via `lastIndexOf('.')` — strips only the final extension. Uploading `Cisco.events.xml` produces stored source name `Cisco.events`. | (no ticket yet) |

**User-visible cascade:**

- `event-source list` prints empty even when sources exist (NMS-19810); use
  `event-source names-and-ids` as a working alternative.
- `find_source_by_name` always reports `Absent` (NMS-19810), so
  `onmsctl apply --diff` shows the whole local document as "added"
  instead of a true delta. The upload itself still succeeds — Horizon's
  upsert path replaces events under an existing basename — so the
  source materializes correctly; treat the diff display as advisory
  until NMS-19810 is fixed upstream.
- `onmsctl apply` and `event-source upload` work today: onmsctl sends
  `name="upload"` on every multipart part (NMS-19813 workaround).
- `onmsctl apply` uploads `kind: EventSource` documents as
  `{metadata.name}.xml` (not `.events.xml`) so Horizon's naive filename
  stripper produces a source name equal to `metadata.name` verbatim.

---

## Reference

### `event-source convert`

`event-source convert` parses each event against the local `EventSource`
schema and emits findings on stderr. Example finding:

```
EC004  error    event missing required field: uei
  At:   bad.events.xml:14:5  (event[3])
  Fix:  Add the required uei to the event in the source XML.
  For the full rationale: onmsctl event-source convert --explain EC004
```

**Finding codes.** `EC001`–`EC008` are stable across releases. Read any
rule's rationale with `onmsctl event-source convert --explain <code>`.

| Code | Severity | Meaning |
|------|----------|---------|
| EC001 | warning | Unmodeled direct-child element under `<event>` dropped on conversion. |
| EC002 | error   | Source has zero events. |
| EC003 | error   | Reserved `metadata.name`. |
| EC004 | error   | Event missing a required field. |
| EC005 | warning | Severity case normalized (e.g. `WARNING` → `Warning`). |
| EC006 | error   | Post-conversion validation failed (catch-all for rules not specifically modeled). |
| EC007 | error   | `alarm-type` outside the accepted set `{1, 2, 3}`. |
| EC008 | error   | Invalid `metadata.name` (disallowed characters). |

**Exit codes.** `0` clean, `1` warnings (YAML written), `2` blocking
findings (no YAML).

**Flags.**

| Flag | Purpose |
|---|---|
| `--format json` | CI envelope with `output` path and `yaml` body. |
| `--max-bytes 64M` | Override the 16 MiB input cap. |
| `--max-findings 0` | Disable the 1000-finding `EC001` cap (set `<n>` for any other limit). |
| `--force` | Overwrite existing output. |
| `--explain <code>` | Print the full rationale for a finding code and exit. |

**Unmodeled elements.** `EC001` is the permanent forward-compatibility
surface: any direct-child element under `<event>` that `onmsctl`'s YAML
schema doesn't model fires `EC001` rather than silently losing data.
The v0.1 modeling gaps (`<snmp>`, `<parameter>`, `<forward>`,
`<script>`, `<filters>`) are now first-class; remaining unmodeled XSD
elements (`<priority>`, `<autoaction>`, `<operaction>`, `<loggroup>`,
vendor extensions) keep firing `EC001` until they're modeled too. For
full fidelity today, keep the XML alongside the YAML and use
`event-source upload`.

`EC001` is **structural-only** — it does not detect attribute extensions
on modeled elements or enum-value drift on modeled fields.

### EventSource YAML schema

#### `alarmType`

`spec.events[].alarmData.alarmType` strictly accepts the three known
states, in either symbolic (Web UI) form or the integer it maps to:

| Symbolic        | Integer |
|-----------------|---------|
| `raise`         | `1`     |
| `resolution`    | `2`     |
| `unresolvable`  | `3`     |

Symbolic input is case-insensitive on parse; the canonical YAML output
is always lowercase. Anything else — unknown symbolic strings
(`"problem"`, the alarmd Java alias) OR integers outside `{1, 2, 3}` —
fails immediately. YAML inputs reject at deserialize time; eventconf
XML inputs to `event-source convert` produce an `EC007` finding at Error
severity (no YAML written, exit 2).

#### `snmp`

`spec.events[].snmp` mirrors the eventconf XSD's `<snmp>` element.
Every sub-field is optional. Practical *numeric* ranges are documented
but NOT enforced — out-of-range integers round-trip verbatim. *String*
fields are rejected when set to empty or whitespace-only.

- `id` — enterprise OID; free string, no OID-format validation.
- `idtext` — vendor-supplied textual label.
- `version` — common values `v1` / `v2c` / `v3` (free string;
  `v3-auth-priv` and other variants accepted verbatim).
- `generic` — `0..=6` per RFC 1157.
- `specific` — `>= 0`.
- `community` — typically `public`.

#### `parameters`

`spec.events[].parameters` mirrors `<parameter name="..." value="..."
expand="..."/>` — *static* per-event configuration eventd attaches to
fired events. Each entry requires `name` and `value`; `expand` is
optional and controls whether eventd substitutes `%parm[#N]%`-style
placeholders at fire time. Document order is preserved.

This is **distinct** from `parmCollection` on a *fired* event instance
(a runtime field on the JSON wire, not modeled here). The two share
similar names but live in different domains and MUST NOT be conflated.

#### `forwards` and `scripts`

`spec.events[].forwards` mirrors `<forward state="..." mechanism="...">
target</forward>` — eventd's forwarding directives. The local schema
validates against the XSD-closed sets:

- `state` ∈ `{on, off}`
- `mechanism` ∈ `{snmpudp, snmptcp, xmltcp, xmludp}`

Values outside these sets are rejected locally (otherwise Horizon
returns a server-side 400). An empty `forwards: [{}]` entry is rejected
too — at least one of `state`, `mechanism`, `target` must be set.

`spec.events[].scripts` mirrors `<script language="...">body</script>` —
embedded executable logic (typically BeanShell) that eventd runs on
event arrival. `language` is REQUIRED per the XSD; `body` is optional
and preserved byte-for-byte. Use YAML's `|` literal block for
multi-line bodies — clip mode (`|`) keeps one trailing newline, strip
mode (`|-`) keeps none.

> **Security note for `scripts:`.** Shipping executable code via
> `onmsctl apply` lowers the friction for deploying server-side code
> execution on Horizon. The underlying threat surface already exists
> at the raw eventconf-XML upload path — modeling `<script>` in YAML
> does not introduce new authority. Operators should ensure RBAC on
> eventconf write access in Horizon reflects this: anyone who can
> upload an event source can run code on the Horizon JVM.

#### `filters`

`spec.events[].filters` mirrors `<filters><filter eventparm="..."
pattern="..." replacement="..."/></filters>`. Each entry is a regex-
replacement rule that eventd applies to a named event parameter at
fire time:

```
Pattern.compile(pattern)
  .matcher(parmValue)
  .replaceAll(replacement)
```

All three fields are required. `pattern` uses Java regex syntax;
`replacement` supports `$1`/`$2`-style backreferences. The YAML is
flat — operators write `filters:` directly on the event; the
`<filters>` wrapper materializes only on XML render.

**`<mask>` vs `<filters>`.** `<mask>` *selects* which events a source
applies to (SNMP PDU shape matching: id / generic / specific / varbind
values). `<filters>` operates *after* selection, transforming
parameter values on the fired event. Two different layers — `<mask>`
is selection, `<filters>` is post-selection parameter rewrite.

### Apply-time limitations

(`onmsctl apply --help` for full text.)

| # | Limitation | Workaround |
|---|---|---|
| 1 | `description` not set/preserved through `apply`. | Carry the source's intent in the YAML and in git review; the field round-trips locally but is not persisted server-side in v0.1. |
| 2 | Disabled-state `apply` has a bounded enabled-flap window. | `--verbose` warns when this runs. |
| 3 | `vendor` is filename-derived, not declared. | Encode as the prefix before the first `.` in `metadata.name`. |
| 4 | `fileOrder` is server-managed in v0.1. | Deferred to a future `kind: EventConfMaster`. |

### Output formats

Every list/get accepts `-o table` (default), `-o yaml`, `-o json`.
Tables go through `comfy-table`; structured outputs use `serde_json`
and `serde_norway`.

### CLI exit codes

Stable per `cli-core` spec §4.5; safe for scripting:

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | HTTP non-success / partial-failure batch / post-upload state-sync failed |
| 2 | misuse / config error / generic internal |
| 3 | reserved (unassigned in v0.1) |
| 4 | DNS resolution failure |
| 5 | connection refused |
| 6 | timeout |
| 7 | TLS handshake failed |
| 8 | redirect loop |
| 9 | unsupported authentication scheme |
| 10 | `--wait` timed out before the async operation completed |
| 11 | `--wait` observed the async operation fail server-side |
| 12 | write refused locally by a `read-only` context |
| 13 | `apply` refused: would empty a protected role (IAM-001 admin lockout) |
| 14 | `apply` refused: would strip the calling user's own protected role / delete their account (IAM-002 self lockout) |
| 15 | `apply` refused: caller identity unavailable via `GET /users/whoami`, so the self-lockout check can't run |

### Shell completions

```sh
# Bash
onmsctl completion bash > /etc/bash_completion.d/onmsctl

# Fish
onmsctl completion fish > ~/.config/fish/completions/onmsctl.fish
```

Zsh target depends on your setup:

| Setup | Target |
|---|---|
| Oh My Zsh | `~/.oh-my-zsh/custom/completions/_onmsctl` |
| Homebrew zsh (macOS) | `"$(brew --prefix)/share/zsh/site-functions/_onmsctl"` |
| System-wide on Linux | `/usr/local/share/zsh/site-functions/_onmsctl` |
| Plain user-local | `~/.zsh/completions/_onmsctl` (then add the dir to `$fpath`) |

```sh
# Oh My Zsh
mkdir -p ~/.oh-my-zsh/custom/completions
onmsctl completion zsh > ~/.oh-my-zsh/custom/completions/_onmsctl
```

Oh My Zsh does not pick up `$ZSH_CUSTOM/completions/` automatically.
Add this to `~/.zshrc` above `event-source $ZSH/oh-my-zsh.sh`:

```sh
fpath=("$ZSH_CUSTOM/completions" $fpath)
```

Then start a new shell or run `compinit`.

**Avoid** `> "${fpath[1]}/_onmsctl"` — on Oh My Zsh `${fpath[1]}` is
typically `~/.oh-my-zsh/plugins/git`, which is both wrong and confusing.

Renaming: the generated script targets the literal binary name
`onmsctl`. Post-process with e.g. `sed -e 's/onmsctl/<name>/g'` if
you've repackaged or symlinked under another name.

### TLS

`server.insecure-skip-tls-verify: true` (or `--insecure-tls`) disables
certificate verification. Every outgoing request then emits:

```
warning: TLS certificate verification is disabled (insecure-skip-tls-verify) for GET request. Use only on trusted networks.
```

Keep this off in production. The path is intentionally omitted from the
warning to avoid log-cardinality explosion and accidental PII retention.

---

## License

Apache-2.0. Third-party crate licenses inventoried in
`THIRD-PARTY-LICENSES.md` (regenerated by `make licenses`).

## Contributing

See `CONTRIBUTING.md`. Implementations work from the OpenAPI document
and black-box observation of a Horizon instance — never from Horizon's
server source — so the result remains an Apache-2.0 clean-room.
