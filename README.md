# onmsctl

A `kubectl`-style command-line interface for [OpenNMS Horizon][horizon] —
declarative `apply -f`, an XML→YAML migrator for legacy eventconf, and
imperative verbs for source / event management.

[horizon]: https://www.opennms.com/horizon/

> **Pre-stability notice.** Releases on `v0.x.y` may break CLI flags, the
> configuration schema, and the `EventSource` YAML schema between minor
> versions. Surfaces stabilize at `v1.0.0`.

---

## GitOps for OpenNMS event configuration

Keep event configuration in git as YAML; push to Horizon declaratively.
Two commands carry the loop — `source convert` brings existing XML in,
`source apply` ships edits out.

### 1. Migrate existing XML → YAML

`source convert` is a pure local file transform — no Horizon contact required:

```sh
onmsctl source convert /opt/opennms/etc/events/cisco.foo.events.xml
onmsctl source convert --output-dir yaml/ /opt/opennms/etc/events/*.events.xml
```

Rule violations emit stable, file:line-anchored findings on stderr:

```
EC004  error    event missing required field: uei
  At:   bad.events.xml:14:5  (event[3])
  Fix:  Add the required uei to the event in the source XML.
  For the full rationale: onmsctl source convert --explain EC004
```

Codes `EC002`–`EC009` are stable across releases (`EC001` / `EC006`
reserved); read any rule's rationale with `--explain <code>`. Exit code
is `0` clean, `1` warnings (YAML written), `2` blocking (no YAML).

Useful flags: `--format json` (CI envelope with `output` path and
`yaml` body), `--max-bytes 64M` (override the 16 MiB input cap),
`--force` (overwrite existing output).

**v0.1 limitation:** unmodeled elements (`<snmp>`, `<script>`,
`<forward>`, `<parameter>`, `<filters>`) are silently dropped. Keep the
XML alongside the YAML and use `source upload` when full fidelity
matters. A future release will surface their presence as `EC006`.

### 2. Apply YAML to Horizon

```sh
onmsctl source apply -f cisco.foo.yaml --diff
```

Fetches the server's current state, prints a UEI-bucketed diff to stderr,
uploads only when changes exist. Add `--dry-run` to simulate.

The `examples/` directory ships fixtures: `minimal.yaml`, `full.yaml`
(every nested type — `mask`, `alarmData`, `logmsg`, `correlation`,
`autoacknowledge`, `tticket`, `mouseovertext`), `severities.yaml`,
`disabled.yaml`.

**Apply-time limitations** (`source apply --help` for full text):

| # | Limitation | Workaround |
|---|---|---|
| 1 | `description` not set/preserved through `apply`. | `source create --description ...` at first creation. |
| 2 | Disabled-state `apply` has a bounded enabled-flap window. | Imperative path for strict avoidance; `--verbose` warns when this runs. |
| 3 | `vendor` is filename-derived, not declared. | Encode as the prefix before the first `.` in `metadata.name`. |
| 4 | `fileOrder` is server-managed in v0.1. | Deferred to a future `kind: EventConfMaster`. |

### Editor integration

A JSON Schema (draft 2020-12) lives at
[`schemas/event-source.schema.json`](schemas/event-source.schema.json).
Add one line at the top of your YAML so
[`yaml-language-server`](https://github.com/redhat-developer/yaml-language-server)-aware
editors validate on save:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/event-source.schema.json
```

Pin to a release tag for stability or reference a clone with
`./schemas/event-source.schema.json`. Regenerate with `make schema`;
CI fails if the committed artifact lags.

---

## Install

Pre-compiled binaries are published as GitHub Releases for every `v*.*.*` tag. Each release ships per-target binaries, per-binary SHA256 checksums, an aggregate `SHA256SUMS` file, and Sigstore (cosign) keyless signatures + certificates for every asset.

Supported targets:

| Target                          | Asset suffix                       |
|---------------------------------|------------------------------------|
| Linux x86_64                    | `x86_64-unknown-linux-gnu`         |
| Linux aarch64                   | `aarch64-unknown-linux-gnu`        |
| macOS x86_64 (Intel)            | `x86_64-apple-darwin`              |
| macOS aarch64 (Apple Silicon)   | `aarch64-apple-darwin`             |

Windows is not yet in the release matrix; Windows users build from source (below).

### Quick path (Linux/macOS)

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

### Verify the cosign signature (recommended)

Every release asset is signed via Sigstore's keyless OIDC flow. Verifying the signature ties the binary you downloaded to a specific GitHub Actions workflow run on this repository, with no long-lived key to compromise.

```sh
cosign verify-blob \
  --certificate-identity-regexp "^https://github.com/no42-org/onmsctl/.github/workflows/release.yml@" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate onmsctl-${VERSION}-${TARGET}.pem \
  --signature  onmsctl-${VERSION}-${TARGET}.sig \
  onmsctl-${VERSION}-${TARGET}
```

The same pattern verifies `SHA256SUMS` and the per-crate CycloneDX SBOM files shipped alongside the binaries.

### macOS Gatekeeper

The binaries are unsigned by Apple (only by Sigstore). On first run macOS may quarantine the file. Either:

```sh
xattr -d com.apple.quarantine /usr/local/bin/onmsctl
```

or open System Settings → Privacy & Security and approve once. This is a one-time prompt per download.

---

## Build from source

For development, unreleased commits, or platforms outside the release matrix (e.g. Windows). Requires the toolchain pinned in `rust-toolchain.toml` (currently Rust 1.95):

```sh
git clone https://github.com/no42-org/onmsctl
cd onmsctl
make build              # debug build → target/debug/onmsctl
cargo build --release   # → target/release/onmsctl
cargo install --path crates/onmsctl   # → ~/.cargo/bin
```

Verify with `onmsctl version`.

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

## Imperative ops

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

Four `/eventconf/*` endpoint bugs reproducible on Horizon 35.0.5 and
36.0.0; all tracked upstream. `onmsctl` works around NMS-19813
client-side.

| # | Endpoint | Symptom | Upstream |
|---|---|---|---|
| 1 | `GET /eventconf/filter/sources` | returns empty `eventConfSourceList` despite non-zero `totalRecords` | [NMS-19810](https://opennms.atlassian.net/browse/NMS-19810) |
| 2 | `GET /eventconf/filter/{id}/events` | HTTP 500 NPE when `offset` is omitted | [NMS-19811](https://opennms.atlassian.net/browse/NMS-19811) |
| 3 | `/eventconf/filter/*` | paging-parameter requirements differ per endpoint, undocumented | [NMS-19812](https://opennms.atlassian.net/browse/NMS-19812) |
| 4 | `POST /eventconf/upload` | HTTP 400 (empty body) unless the multipart field name is literally `upload` — CXF `@Multipart("upload")` qualifier on the JAX-RS interface | [NMS-19813](https://opennms.atlassian.net/browse/NMS-19813) |

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

---

## Reference

### Output formats

Every list/get accepts `-o table` (default), `-o yaml`, `-o json`.
Tables go through `comfy-table`; structured outputs use `serde_json`
and `serde_norway`.

### Exit codes

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
