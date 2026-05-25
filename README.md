# onmsctl

A `kubectl`-style command-line interface for [OpenNMS Horizon][horizon] —
declarative `apply -f` for event configuration and provisioning
requisitions, XML→YAML migrators for both legacy eventconf and
`provision.pl`-shape requisition files, and imperative verbs for
source / event / requisition management.

[horizon]: https://www.opennms.com/horizon/

> **Pre-stability notice.** Releases on `v0.x.y` may break CLI flags, the
> configuration schema, and the `EventSource` YAML schema between minor
> versions. Surfaces stabilize at `v1.0.0`.

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
```

Each capability owns its own subcommand tree (`source` / `event`,
`requisition`, etc.) and ships its own JSON Schemas, validators, and
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

## GitOps for OpenNMS event configuration

Keep event configuration in git as YAML; push to Horizon declaratively.
Two commands carry the loop — `source convert` brings existing XML in,
`source apply` ships edits out.

### Step 1 — Convert existing XML → YAML

Pure local file transform; no Horizon contact required.

```sh
onmsctl source convert /opt/opennms/etc/events/cisco.foo.events.xml
onmsctl source convert --output-dir yaml/ /opt/opennms/etc/events/*.events.xml
```

Rule violations emit stable, file:line-anchored findings on stderr (see
[`source convert` reference](#source-convert) for the finding-code
catalog, exit codes, flags, and the unmodeled-element policy).

### Step 2 — Apply YAML to Horizon

```sh
onmsctl source apply -f cisco.foo.yaml --diff
```

Fetches the server's current state, prints a UEI-bucketed diff to
stderr, uploads only when changes exist. Add `--dry-run` to simulate.

`source apply -f` accepts a single file, a directory of YAML files, or
a glob pattern (quote it so the shell doesn't pre-expand):

```sh
onmsctl source apply -f sources/                  # directory
onmsctl source apply -f 'sources/cisco-*.yaml'    # glob
```

With multiple files, each is applied in alphabetical (byte-wise) order
with continue-on-error semantics — failures are collected and the
exit code is non-zero if any file failed, but later files still run.
`--diff` is single-file-only, but a glob that matches exactly one
file collapses to single-file mode so `--diff` still applies.

`metadata.name` becomes the server's stored source name verbatim;
Horizon derives `vendor` server-side as the prefix before the first `.`
(so `metadata.name: cisco.foo` → source name `cisco.foo`, vendor `cisco`).

The `examples/` directory ships fixtures: `minimal.yaml`, `full.yaml`
(every nested type), `severities.yaml`, `disabled.yaml`. Full schema
detail in the [EventSource YAML reference](#eventsource-yaml-schema).

### Step 3 — Iterate

Edit YAML in git, push through review, run `source apply -f` again. The
diff display flags changes the upload would make. `--dry-run` is safe
for any branch; `source apply` itself is idempotent (Horizon's upsert
path replaces events under an existing basename).

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
from git. Three verbs carry the loop — `requisition convert` brings
existing XML in, `requisition apply` ships edits out, `requisition
status` / `import` cover the lifecycle.

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

### Step 2 — Apply YAML to Horizon

```sh
# Single file — preview without writing. --diff goes to stderr,
# structured outcome to stdout. Safe against any context.
onmsctl requisition apply -f acme-prod.yaml --dry-run --diff

# Single file — real apply. With --wait, blocks until import
# completes (or --timeout fires for exit 10).
onmsctl requisition apply -f acme-prod.yaml --wait --timeout 5m

# Directory mode — every *.yaml / *.yml under the path is applied
# in alphabetical order. Phase 1 cross-file collision check runs
# first (duplicate metadata.name aborts before any writes;
# duplicate foreignId warns and continues). --stop-on-error
# (kubectl-style) halts phase 2 after the first per-file failure;
# default is continue-on-error.
onmsctl requisition apply -f ./requisitions/ --dry-run
onmsctl requisition apply -f ./requisitions/ --stop-on-error --wait
```

The apply path computes a three-level diff (canonical-bytes / per-node
/ per-leaf) and auto-decides `rescanExisting` from the scan-relevance
of what changed. Override with `--rescan-existing true|false`.

In directory mode, `--wait` polls per-file after each successful
apply (not as one batch wait). `--diff` is single-file only — for
directory previews use `--dry-run -o yaml` to see the structured
outcomes per file.

### Step 3 — Iterate

Edit YAML in git, push through review, run `requisition apply -f`
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
| `provision.pl requisition add <fs>` | `onmsctl requisition apply -f <fs>.yaml` (with an empty `nodes: []` payload) |
| `provision.pl requisition remove <fs>` | `onmsctl requisition delete <fs> --yes` (issues both `DELETE /rest/requisitions/<fs>` AND `DELETE /rest/requisitions/deployed/<fs>` in one call; idempotent on both — 404 on either snapshot is treated as success. **`--yes` is required in non-TTY contexts (CI / scripting); TTY contexts prompt interactively.** Remove the local YAML separately) |
| `provision.pl requisition import <fs>` | `onmsctl requisition import <fs>` (PUT-only, no re-POST; add `--wait` to block until completion) |
| `provision.pl requisition list` | `onmsctl requisition list` (wraps `GET /rest/requisitionNames`; respects `-o` table / json / yaml) |
| `provision.pl node add <fs> <foreign-id> <node-label>` | Recommended: edit YAML, `onmsctl requisition apply -f <fs>.yaml`. Imperative escape-hatch: `onmsctl requisition node add <fs> <foreign-id> --label <node-label>` (the imperative path stages the change in the pending requisition; run `requisition import <fs>` to take effect, or `apply -f` to also resync the rest of the YAML) |
| `provision.pl interface add <fs> <foreign-id> <ip>` | Recommended: edit YAML, `onmsctl requisition apply -f <fs>.yaml`. Imperative escape-hatch: `onmsctl requisition interface add <fs> <foreign-id> <ip> --snmp-primary P\|S\|N [--status 1\|3] [--descr ...]`. Sibling verbs: `interface list / get / set / remove`. Stages the change in the pending requisition; run `requisition import <fs>` to take effect. **Note:** `--status` and `--descr` are wire-only — apply→export→apply silently drops those values |
| `provision.pl service add <fs> <foreign-id> <ip> <svc>` | Recommended: edit YAML, `onmsctl requisition apply -f <fs>.yaml`. Imperative escape-hatch: `onmsctl requisition service add <fs> <foreign-id> <ip> <svc>`. Sibling verbs: `service list / remove`. Stages the change in the pending requisition; run `requisition import <fs>` to take effect |
| `provision.pl category add <fs> <foreign-id> <cat>` | Recommended: edit YAML, `onmsctl requisition apply -f <fs>.yaml`. Imperative escape-hatch: `onmsctl requisition category add <fs> <foreign-id> <cat>`. Sibling verbs: `category list / remove`. Stages the change in the pending requisition; run `requisition import <fs>` to take effect |
| `provision.pl asset add <fs> <foreign-id> <name> <value>` | Recommended (requisition-time): edit YAML's `spec.nodes[].assets`, `onmsctl requisition apply -f <fs>.yaml`. Imperative escape-hatch (post-import, takes effect immediately): `onmsctl requisition asset set <db-id> <field> <value>`. Sibling verbs: `asset list / get`. **Misfit:** keyed by integer database node ID, not foreign-id — resolve via `curl /opennms/rest/nodes?foreignId=<fid>` before running |

The migration philosophy reverses `provision.pl`'s shell-automation
model. Where `provision.pl` ran one mutation per invocation,
`onmsctl requisition apply` ships the desired state and lets the
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
   requisition apply -f <fs>.yaml` invocations. The YAML carries
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

## Aliases and read-only contexts

### Top-level verb aliases

Every capability registers a short alias so the most common verbs
stay terse:

| Long form | Alias | Notes |
|---|---|---|
| `onmsctl source ...` | `onmsctl src ...` | eventconf source verbs |
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

Verbs classified `WriteCmd` at compile time (`source create / delete /
apply`, `event add / update / delete`, `requisition apply / import`)
refuse with exit code 12 against a read-only context. Reads pass
through. The `--read-only` flag and `$ONMSCTL_READ_ONLY` env var
force the flag on regardless of context — precedence is **flag > env >
context > default false**, and the flag is one-way (no `--no-read-only`
escape hatch — context can never un-set it).

---

## Imperative operations

For ad-hoc work outside the GitOps loop:

```sh
# sources
onmsctl source list                  # -o table | -o yaml | -o json
onmsctl source get 42
onmsctl source create --name acme.widget --description "Acme widget events"
onmsctl source delete 42 43
onmsctl source enable 42 --cascade
onmsctl source disable 42
onmsctl source names                 # name-only listing
onmsctl source names-and-ids

# events (refs are <source-id>/<event-id>)
onmsctl event list --source 42
onmsctl event list --uei "uei.opennms.org/vendor/cisco/.*"   # cross-source
onmsctl event list --vendor cisco
onmsctl event add --source 42 -f examples/single-event.yaml
onmsctl event update 42/108 -f event.yaml --enabled true     # --enabled required
onmsctl event delete 42/108 42/109
onmsctl event enable 42/108
onmsctl event disable 42/108

# raw XML round-trip
onmsctl source upload cisco.foo.events.xml acme.widget.events.xml
onmsctl source download 42 -O cisco.foo.events.xml
onmsctl source download 42 --format yaml -O cisco.foo.yaml   # convert inline
```

`event add` / `event update` expect a single Event payload; see
`examples/single-event.yaml`. `source download → edit → apply` may drop
server-only fields not modeled locally — keep the XML alongside for
full fidelity.

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

- `source list` prints empty even when sources exist (NMS-19810); use
  `source names-and-ids` as a working alternative.
- `find_source_by_name` always reports `Absent` (NMS-19810), so
  `source apply --diff` shows the whole local document as "added"
  instead of a true delta. The upload itself still succeeds — Horizon's
  upsert path replaces events under an existing basename — so the
  source materializes correctly; treat the diff display as advisory
  until NMS-19810 is fixed upstream.
- `source apply` and `source upload` work today: onmsctl sends
  `name="upload"` on every multipart part (NMS-19813 workaround).
- `source apply` uploads as `{metadata.name}.xml` (not `.events.xml`)
  so Horizon's naive filename stripper produces a source name equal
  to `metadata.name` verbatim.

---

## Reference

### `source convert`

`source convert` parses each event against the local `EventSource`
schema and emits findings on stderr. Example finding:

```
EC004  error    event missing required field: uei
  At:   bad.events.xml:14:5  (event[3])
  Fix:  Add the required uei to the event in the source XML.
  For the full rationale: onmsctl source convert --explain EC004
```

**Finding codes.** `EC001`–`EC008` are stable across releases. Read any
rule's rationale with `onmsctl source convert --explain <code>`.

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
`source upload`.

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
XML inputs to `source convert` produce an `EC007` finding at Error
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
> `source apply` lowers the friction for deploying server-side code
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

(`source apply --help` for full text.)

| # | Limitation | Workaround |
|---|---|---|
| 1 | `description` not set/preserved through `apply`. | `source create --description ...` at first creation. |
| 2 | Disabled-state `apply` has a bounded enabled-flap window. | Imperative path for strict avoidance; `--verbose` warns when this runs. |
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
Add this to `~/.zshrc` above `source $ZSH/oh-my-zsh.sh`:

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
