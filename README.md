# onmsctl

A `kubectl`-style command-line interface for [OpenNMS Horizon][horizon] —
declarative `apply -f`, imperative source / event verbs, and live
introspection — talking to the EventConf REST API introduced in Horizon 36.

[horizon]: https://www.opennms.com/horizon/

> **Pre-stability notice.** Releases on `v0.x.y` may break CLI flags, the
> configuration schema, and the `EventSource` YAML schema between minor
> versions. Surfaces stabilize at `v1.0.0`.

---

## Install

### From source

Requires the toolchain pinned in `rust-toolchain.toml` (currently Rust
1.95). On a workstation that has `rustup`:

```sh
git clone https://github.com/no42-org/onmsctl
cd onmsctl
make build              # debug build at target/debug/onmsctl
cargo build --release   # optimized build at target/release/onmsctl
cargo install --path crates/onmsctl   # install into ~/.cargo/bin
```

### Verify

```sh
onmsctl version
# onmsctl 0.1.0
# capabilities:
#   - eventconf 0.1.0
```

---

## Configure a context

`onmsctl` follows the kubectl pattern: one config file, one or more
named contexts, one currently-active context.

Default config path:

| OS      | Path |
|---------|------|
| Linux   | `$XDG_CONFIG_HOME/onmsctl/config.yaml` (typically `~/.config/onmsctl/config.yaml`) |
| macOS   | `~/Library/Application Support/org.no42-org.onmsctl/config.yaml` |
| Windows | `%APPDATA%\no42-org\onmsctl\config\config.yaml` |

Override with `--config <path>` or `$ONMSCTL_CONFIG`.

### Minimal example

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

### Credential references

`auth.basic` accepts exactly one of:

| Field           | Example                                       | Notes |
|-----------------|-----------------------------------------------|-------|
| `password`      | `password: hunter2`                           | Inline. Convenient, plain-text on disk. |
| `password-file` | `password-file: /run/secrets/onms-prod`       | Mode `0600` recommended; trailing newlines stripped. |
| `keyring`       | `keyring: { service: onmsctl, account: prod }` | OS keyring (macOS Keychain, Windows Credential Manager, Linux Secret Service via `keyring` crate). |

`auth.bearer` accepts the same shapes with `token` / `token-file` /
`keyring`.

Resolution order at request time:

```
env ($ONMS_PASSWORD / $ONMS_TOKEN)  >  keyring  >  file  >  inline
```

### Switch contexts

```sh
onmsctl config view                  # current config (secrets redacted)
onmsctl config use-context staging   # rewrite current-context atomically
```

`config view` redacts inline `password` / `token` strings; `password-file`
/ `token-file` / `keyring` references remain visible (they are pointers,
not secrets).

`config use-context` resolves symlinks before writing, so a symlinked
`config.yaml` writes through to the upstream file rather than being
replaced.

---

## Override precedence

```
flags (--url, --user, --context)  >  env (ONMS_URL, ONMS_USER, ONMSCTL_CONTEXT)  >  active context  >  built-in default
```

---

## Four canonical workflows

### 1. Apply (declarative)

Author an `EventSource` document, then ship it:

```sh
onmsctl source apply -f examples/full.yaml --diff
```

The CLI fetches the server's current state, computes a structured
UEI-bucketed diff (additions / removals / modifications), prints it to
stderr, and uploads only when changes exist. Add `--dry-run` to simulate
without issuing mutations.

The `examples/` directory ships fixtures covering every nested type the
schema models:

| Fixture                    | What it shows |
|----------------------------|---------------|
| `examples/minimal.yaml`    | Smallest valid document. |
| `examples/full.yaml`       | Every nested type: `mask` (with `elements` and `varbinds`), `alarmData`, `logmsg`, `correlation`, `autoacknowledge`, `tticket`, `mouseovertext`. |
| `examples/severities.yaml` | One event per severity level (`Indeterminate`, `Cleared`, `Normal`, `Warning`, `Minor`, `Major`, `Critical`). |
| `examples/disabled.yaml`   | `spec.enabled: false` — exercises the apply-then-PATCH disable path. |

**Known limitations** (run `onmsctl source apply --help` for full text):

| # | Limitation | Workaround |
|---|---|---|
| 1 | `description` cannot be set or preserved through `apply`. | `onmsctl source create --description ...` at first creation. |
| 2 | Disabled-state `apply` has a bounded enabled-flap window. | Use the imperative path for strict avoidance; `--verbose` emits a warning when this path runs. |
| 3 | `vendor` is filename-derived, not declared. | Encode the vendor as the prefix before the first `.` in `metadata.name`. |
| 4 | `fileOrder` is server-managed in v0.1. | Declarative ordering deferred to a future `kind: EventConfMaster`. |

### 2. Imperative source ops

```sh
onmsctl source list                                           # default -o table
onmsctl source list -o json
onmsctl source get 42
onmsctl source create --name acme.widget --description "Acme widget events"
onmsctl source delete 42 43
onmsctl source enable 42 --cascade
onmsctl source disable 42
onmsctl source names                                          # name-only listing
onmsctl source names-and-ids
```

### 3. Imperative event ops

Events live under a source. Refs use `<source-id>/<event-id>`:

```sh
onmsctl event list --source 42
onmsctl event list --uei "uei.opennms.org/vendor/cisco/.*"    # cross-source UEI filter
onmsctl event list --vendor cisco                             # vendor-scoped
onmsctl event add --source 42 -f event.yaml
onmsctl event update 42/108 -f event.yaml --enabled true      # --enabled is required
onmsctl event delete 42/108 42/109
onmsctl event enable 42/108
onmsctl event disable 42/108
```

### 4. Upload / download (XML round-trip)

```sh
onmsctl source upload cisco.foo.events.xml acme.widget.events.xml
onmsctl source download 42 -O cisco.foo.events.xml
onmsctl source download 42 | tee /tmp/cisco.foo.events.xml    # stdout streaming
```

> **Round-trip caveat:** `download → edit → apply` may drop server-only
> fields the local DTOs don't model. The XML is the authoritative form;
> the YAML is a curated subset. See `source apply --help`.

---

## Output formats

Every list/get accepts `-o table` (default), `-o yaml`, `-o json`. Tables
go through `comfy-table`; structured outputs use `serde_json` and
`serde_norway`.

---

## Exit codes

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

---

## Shell completions

```sh
# Bash
onmsctl completion bash > /etc/bash_completion.d/onmsctl

# Zsh
onmsctl completion zsh > "${fpath[1]}/_onmsctl"

# Fish
onmsctl completion fish > ~/.config/fish/completions/onmsctl.fish
```

The generated script targets the literal binary name `onmsctl`. If
you've repackaged or symlinked the binary under a different name,
post-process the output (e.g. `sed -e 's/onmsctl/<name>/g'`).

---

## TLS

`server.insecure-skip-tls-verify: true` (or `--insecure-tls`) disables
certificate verification. When set, every outgoing request emits a
warning to stderr:

```
warning: TLS certificate verification is disabled (insecure-skip-tls-verify) for GET request. Use only on trusted networks.
```

Keep this off in production. The path is intentionally not included in
the warning to avoid log-cardinality explosion and accidental PII
retention.

---

## Server compatibility

| Server                  | Status                                                                 |
|-------------------------|------------------------------------------------------------------------|
| OpenNMS Horizon **36+** | Primary target. The EventConf REST surface this CLI consumes was introduced here. |

---

## License

Apache-2.0. Third-party crate licenses inventoried in
`THIRD-PARTY-LICENSES.md` (regenerated by `make licenses`).

---

## Contributing

See `CONTRIBUTING.md`. Implementations work from the OpenAPI document
and black-box observation of a Horizon instance — never from Horizon's
server source — so the result remains an Apache-2.0 clean-room.
