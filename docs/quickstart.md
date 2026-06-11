# onmsctl Quick Start

A practical, end-to-end guide to installing, configuring, and using
`onmsctl` — the command-line interface for OpenNMS Horizon.

`onmsctl` follows the kubectl pattern: one config file with named
contexts, a single declarative `apply -f` mutation entrypoint, and
read-only inspection verbs alongside it. It is a single statically
linked binary that bundles three capabilities — **eventconf**
(`event-source` / `event`), **provisioning** (`requisition`), and **IAM**
(`iam`).

> This is the fast path. For signature verification, the full
> `provision.pl` migration map, the EventSource schema reference, and
> server-compatibility notes, see the [README](../README.md).

## Contents

- [1. Prerequisites](#1-prerequisites)
- [2. Install](#2-install)
- [3. Configure a context](#3-configure-a-context)
- [4. Core concepts](#4-core-concepts)
- [5. Five-minute tour](#5-five-minute-tour)
- [6. GitOps for event configuration](#6-gitops-for-event-configuration)
- [7. GitOps for provisioning requisitions](#7-gitops-for-provisioning-requisitions)
- [8. Managing IAM users](#8-managing-iam-users)
- [9. Global flags and environment variables](#9-global-flags-and-environment-variables)
- [10. Output formats and exit codes](#10-output-formats-and-exit-codes)
- [11. Shell completion](#11-shell-completion)
- [12. Troubleshooting](#12-troubleshooting)

---

## 1. Prerequisites

- A reachable OpenNMS Horizon instance and its base URL
  (e.g. `https://horizon.dev.lab/opennms`).
- A Horizon user with the rights for what you intend to do. Read verbs
  need read access; `apply` and other write verbs need a role that can
  mutate the relevant resource (admin for IAM).
- To build from source: the Rust toolchain pinned in
  `rust-toolchain.toml`.

## 2. Install

### Pre-compiled binary (recommended)

Binaries are published as GitHub Releases for Linux and macOS
(x86_64 + aarch64). Windows builds from source.

```sh
VERSION=v0.1.0
TARGET=x86_64-apple-darwin   # or *-unknown-linux-gnu, aarch64-apple-darwin, …

curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}
curl -fL -O https://github.com/no42-org/onmsctl/releases/download/${VERSION}/onmsctl-${VERSION}-${TARGET}.sha256

shasum -a 256 -c onmsctl-${VERSION}-${TARGET}.sha256
chmod +x onmsctl-${VERSION}-${TARGET}
sudo mv onmsctl-${VERSION}-${TARGET} /usr/local/bin/onmsctl
```

Every release asset is also Sigstore-signed (cosign keyless). Verifying
the signature is recommended — see the
[Install section of the README](../README.md#install) for the
`cosign verify-blob` invocation and the macOS Gatekeeper note.

### Build from source

```sh
git clone https://github.com/no42-org/onmsctl
cd onmsctl
make build                              # debug → target/debug/onmsctl
cargo build --release                   # → target/release/onmsctl
cargo install --path crates/onmsctl     # → ~/.cargo/bin/onmsctl
```

### Verify

```sh
onmsctl version
```

```
onmsctl 0.0.1
capabilities:
  - eventconf 0.0.1
  - provisioning 0.0.1
  - iam 0.0.1
```

The capability list grows as the binary links new capability crates.

## 3. Configure a context

`onmsctl` reads one YAML config file holding one or more named
contexts and a `current-context` pointer.

| OS      | Default config path |
|---------|---------------------|
| Linux   | `$XDG_CONFIG_HOME/onmsctl/config.yaml` (typically `~/.config/onmsctl/config.yaml`) |
| macOS   | `~/Library/Application Support/org.no42-org.onmsctl/config.yaml` |
| Windows | `%APPDATA%\no42-org\onmsctl\config\config.yaml` |

Override the path with `--config <path>` or `$ONMSCTL_CONFIG`.

A minimal config with a single context:

```yaml
current-context: dev
contexts:
  - name: dev
    server:
      url: https://horizon.dev.lab/opennms
    auth:
      basic:
        username: admin
        password: admin        # inline — fine for a throwaway lab, not for git
```

### Credentials without inline secrets

`auth.basic` takes exactly one of `password` / `password-file` /
`keyring`. `auth.bearer` takes `token` / `token-file` / `keyring`.

| Field | Notes |
|---|---|
| `password` / `token` | Inline plain-text. Convenient; leaks if the config leaks. |
| `password-file` / `token-file` | Path to a file; mode `0600` recommended, trailing newline stripped. |
| `keyring` | OS keyring (macOS Keychain / Windows Credential Manager work out of the box; Linux GNOME Keyring/KWallet needs a rebuild — see README). |

```yaml
contexts:
  - name: prod
    server:
      url: https://horizon.example.com/opennms
    auth:
      basic:
        username: automation
        password-file: ~/.secrets/onms-prod   # pointer, safe to commit the config
```

At request time the password/token is resolved in this order:

```
env ($ONMS_PASSWORD / $ONMS_TOKEN)  >  keyring  >  file  >  inline
```

### Inspect and switch contexts

```sh
onmsctl config view                  # print the loaded config, secrets redacted
onmsctl config use-context prod      # atomically rewrite current-context
```

`config view` redacts inline `password` / `token` values; file and
keyring references stay visible because they are pointers, not secrets.

### Verify connectivity

```sh
onmsctl iam whoami                   # confirms URL + credentials work
```

## 4. Core concepts

**Declarative `apply -f` is the one mutation entrypoint.** It peeks
each YAML document's `kind` and routes it to the right handler — there
is no per-capability apply verb. Three kinds are recognized:

| `kind` | `apiVersion` | Reconciles |
|---|---|---|
| `EventSource` | `eventconf.opennms.org/v1` | event configuration sources |
| `Requisition` | `provisioning.opennms.org/v1` | provisioning requisitions |
| `User` | `onmsctl.no42.org/v1alpha1` | Horizon users + roles |

A single file may hold many `---`-separated documents, and a directory
may mix all three kinds.

**Plan → gate → execute.** Every document is planned first. If *any*
document fails to plan (unknown `kind`, duplicate `metadata.name`, parse
error), the whole apply aborts **before** any mutation. Then documents
execute in a fixed kind-precedence order, stopping at the first failure
unless you pass `--continue-on-error`.

**`--dry-run` is always safe.** It plans and prints but issues no
mutating HTTP, so it is allowed even in a read-only context. Pair it
with `--diff` to see exactly what would change.

**Idempotent.** Re-running the same input is the recovery path — an
unchanged document reconciles to "no change" and skips the write.

**Read-only contexts.** A context can set `read-only: true`, or you can
pass `--read-only` (or set `ONMSCTL_READ_ONLY`). Any write verb is then
refused locally before any HTTP call (exit code `12`) — defense in depth
on top of the server's own role checks.

## 5. Five-minute tour

```sh
# 1. Who am I, and does my config work?
onmsctl iam whoami

# 2. What's on the server right now? (read-only)
onmsctl event-source list
onmsctl requisition list
onmsctl iam user list

# 3. Preview a change without touching the server.
onmsctl apply -f my-resource.yaml --dry-run --diff

# 4. Apply it for real.
onmsctl apply -f my-resource.yaml

# 5. Apply an entire directory of mixed resources.
onmsctl apply -f ./gitops/ --recursive
```

`--dry-run --diff` prints the rendered diff to **stderr** and a
structured outcome to **stdout**, so you can review interactively or
pipe the outcome to a tool.

## 6. GitOps for event configuration

Bring existing eventconf XML under version control as YAML, then manage
it declaratively.

### Convert existing XML → YAML

```sh
# Single file to stdout
onmsctl event-source convert mevents.xml > mevents.yaml

# Read from stdin (single input; --name required)
cat mevents.xml | onmsctl event-source convert - --name my-source > my-source.yaml

# Batch a directory into per-input YAML files
onmsctl event-source convert ./xml/*.xml --output-dir ./yaml/
```

Conversion emits `EC###`-coded findings on stderr for anything the YAML
model doesn't represent; run `onmsctl event-source convert --explain <code>`
for the rationale.

### Apply to Horizon

```sh
onmsctl apply -f my-source.yaml --dry-run --diff   # preview
onmsctl apply -f my-source.yaml                     # apply
```

### Inspect and download

```sh
onmsctl event-source list                      # filter / sort / page
onmsctl event-source get <id>
onmsctl event-source download <id> -O out.xml  # round-trip the raw XML
onmsctl event list --source <id>         # events for a source
```

## 7. GitOps for provisioning requisitions

The composite `kind: Requisition` document carries both the requisition
(nodes / interfaces / services / categories / assets) **and** its
optional `spec.foreignSource` (scan interval, detectors, policies).

A node may declare a `location` (its monitoring / Minion location);
omit it for the Default location.

```yaml
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: acme-prod
spec:
  # Omit this whole block for "portable" YAML that inherits Horizon's
  # default foreign-source.
  foreignSource:
    scanInterval: 1d
    detectors:
      - name: ICMP
        class: org.opennms.netmgt.provision.detector.icmp.IcmpDetector
  nodes:
    - foreignId: bbone-sw01
      label: bbone-sw01
      location: labmonkeys-hq          # monitoring (Minion) location
      interfaces:
        - ip: 192.168.8.8
          snmpPrimary: P
          services: [ICMP, SNMP]
      categories: [Production, Network]
```

A complete, every-field example lives at
[`examples/requisition-acme-prod.yaml`](../examples/requisition-acme-prod.yaml).

### Convert or export to YAML

```sh
# Migrate provision.pl-shape XML (requisitions + matching foreign-sources)
onmsctl requisition convert --from ./reqs/ --foreign-sources-dir ./fs/ --out ./yaml/

# Export what's already deployed (reverse of apply)
onmsctl requisition export acme-prod > acme-prod.yaml      # one, to stdout
onmsctl requisition export --out ./yaml/                   # all, per-file
onmsctl requisition export acme-prod --include-defaults    # inline the default FS
```

### Apply and drive the lifecycle

```sh
onmsctl apply -f acme-prod.yaml --dry-run --diff   # preview the per-node diff
onmsctl apply -f acme-prod.yaml                     # POST + auto-import

onmsctl requisition status acme-prod                # deployed state
onmsctl requisition import acme-prod                # re-import without re-POST
onmsctl requisition import acme-prod --rescan-existing   # re-evaluate existing nodes
onmsctl requisition delete acme-prod --yes          # purge pending + deployed
```

`apply` picks `rescanExisting` automatically from the diff: changes that
affect what provisiond discovers (services, SNMP primary, detectors,
**location**) trigger a rescan; pure metadata (labels, categories,
assets) does not.

### Read-only inspection

```sh
onmsctl requisition node list acme-prod
onmsctl requisition interface list acme-prod <foreign-id>
onmsctl requisition service list acme-prod <foreign-id> <ip>
onmsctl requisition category list acme-prod <foreign-id>
```

## 8. Managing IAM users

```yaml
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata:
  name: jdoe
spec:
  fullName: Jane Doe
  email: jane@example.com
  roles: [ROLE_USER]
  passwordRef:            # passwords are create-only and never inline
    fromEnv: JDOE_PASSWORD
```

```sh
JDOE_PASSWORD=… onmsctl apply -f user.yaml --dry-run --diff
JDOE_PASSWORD=… onmsctl apply -f user.yaml
```

See [`examples/iam-user.yaml`](../examples/iam-user.yaml) for a fuller
document. Note `passwordRef` is only used on **create** — applying an
existing user never re-sends the password.

### Read, delete, rotate

```sh
onmsctl iam whoami
onmsctl iam user list
onmsctl iam user get jdoe
onmsctl iam user export > users.yaml          # snapshot all users as YAML
onmsctl iam user delete jdoe

# Rotate a password (pick exactly one source)
onmsctl iam user set-password jdoe --password-stdin   # read one line from stdin
onmsctl iam user set-password jdoe --from-env JDOE_PASSWORD
onmsctl iam user set-password jdoe --from-file ./pw
onmsctl iam user set-password jdoe --from-keyring myservice/jdoe
```

`apply` refuses changes that would empty a protected role (admin
lockout) or strip your own protected role / delete your own account
(self-lockout) — see exit codes `13`–`15`.

## 9. Global flags and environment variables

These work on (almost) every command:

| Flag | Env | Purpose |
|---|---|---|
| `--config <path>` | `ONMSCTL_CONFIG` | Config file path |
| `--context <name>` | `ONMSCTL_CONTEXT` | Active context |
| `--url <url>` | `ONMS_URL` | Server URL override |
| `--user <name>` | `ONMS_USER` | Basic-auth username override |
| `--read-only` | `ONMSCTL_READ_ONLY` | Refuse write verbs locally |
| `-o, --output <fmt>` | — | `table` (default), `yaml`, or `json` |
| `--insecure-tls` | — | Skip TLS verification (avoid in prod) |
| `-v, --verbose` | — | Full error chains + extra diagnostics |
| — | `ONMS_PASSWORD` / `ONMS_TOKEN` | Highest-priority credential source |

`apply` adds `-f/--filename`, `--dry-run`, `--diff`,
`--continue-on-error` (alias `--keep-going`), and `-R/--recursive`.

Override precedence (highest wins):

```
flags  >  environment  >  active context  >  built-in default
```

Top-level verbs have short aliases: `event-source`→`evtsrc`, `event`→`evt`,
`requisition`→`req`, `config`→`cfg`.

## 10. Output formats and exit codes

Pick a machine-readable format for scripting:

```sh
onmsctl event-source list -o json | jq .
onmsctl requisition export acme-prod -o yaml
```

Exit codes are stable and safe to branch on:

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | HTTP non-success / partial-failure batch / post-upload state-sync failed |
| 2 | misuse / config error / generic internal |
| 4 | DNS resolution failure |
| 5 | connection refused |
| 6 | timeout |
| 7 | TLS handshake failed |
| 8 | redirect loop |
| 9 | unsupported authentication scheme |
| 10 | `--wait` timed out before the async operation completed |
| 11 | `--wait` observed the async operation fail server-side |
| 12 | write refused locally by a `read-only` context |
| 13 | `apply` refused: would empty a protected role (admin lockout) |
| 14 | `apply` refused: would strip the caller's own protected role / account (self-lockout) |
| 15 | `apply` refused: caller identity unavailable, so the self-lockout check can't run |

## 11. Shell completion

```sh
# Bash
onmsctl completion bash > /etc/bash_completion.d/onmsctl

# Zsh (Homebrew on macOS, for example)
onmsctl completion zsh > "$(brew --prefix)/share/zsh/site-functions/_onmsctl"

# Fish
onmsctl completion fish > ~/.config/fish/completions/onmsctl.fish
```

Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.

## 12. Troubleshooting

- **`onmsctl version` shows the wrong/old binary** — check `which onmsctl`;
  a release binary in `/usr/local/bin` may shadow a `cargo install` one in
  `~/.cargo/bin` (or vice versa).
- **Auth failures** — confirm with `onmsctl iam whoami`. Remember the
  resolution order: `$ONMS_PASSWORD`/`$ONMS_TOKEN` beats keyring beats
  file beats inline. A stale env var can silently override the config.
- **Wrong server** — `--url`/`$ONMS_URL` override the active context.
  Run `onmsctl config view` to see what's actually loaded.
- **TLS handshake failed (exit 7)** — for a lab with a self-signed cert,
  `--insecure-tls` skips verification (never in production).
- **A write "did nothing"** — you're likely in a read-only context
  (exit `12`) or it was a `--dry-run`. Drop `--dry-run` / `--read-only`.
- **Unexpected diff on re-apply** — run `apply --dry-run --diff` and
  inspect the leaves; cosmetic reordering of set-like fields (categories,
  services) is normalized away, so a real diff means real drift.
- **See the full error chain** — add `-v`.

---

### Where to go next

- [README](../README.md) — full reference: install signing,
  `provision.pl` migration map, EventSource schema, server-compat notes.
- [`examples/`](../examples/) — ready-to-edit YAML for every kind.
- [`schemas/`](../schemas/) — JSON Schemas for editor validation; add the
  `# yaml-language-server: $schema=…` directive to your YAML for
  in-editor completion and validation.
- [`docs/manual-test-runbook.md`](manual-test-runbook.md) — local
  end-to-end verification steps.
