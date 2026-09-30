---
title: Global flags and environment variables
description: Flags and environment variables that work across onmsctl commands, and their override precedence.
---

These flags work on every command.

| Name | Type | Default | Env | Description |
|---|---|---|---|---|
| `--config` | path | Linux: `$XDG_CONFIG_HOME/onmsctl/config.yaml` (typically `~/.config/onmsctl/config.yaml`). macOS: `~/Library/Application Support/org.no42-org.onmsctl/config.yaml`. Windows: `%APPDATA%\no42-org\onmsctl\config\config.yaml`. | `ONMSCTL_CONFIG` | Config file path. |
| `--context` | name | the config's `current-context` | `ONMSCTL_CONTEXT` | Active context. |
| `--url` | URL | the context's `server.url` | `ONMS_URL` | Server URL override. |
| `--user` | name | the context's username | `ONMS_USER` | Basic-auth username override. |
| `--read-only` | switch | off | `ONMSCTL_READ_ONLY` | Refuse write verbs locally. The flag and the variable only turn read-only on. The variable accepts `1`, `true`, `yes`, `on`. |
| `-o`, `--output` | `table` \| `yaml` \| `json` | **`table`** | none | Output format. See [output formats](output-formats.md). |
| `--insecure-tls` | switch | off | **none** | Skip TLS certificate verification. Avoid in production. It **cannot re-enable** verification for a context that disables it. See [TLS](tls.md). |
| `-v`, `--verbose` | switch | off | none | Full error chains plus extra diagnostics. |
| `-V`, `--version` | switch | | none | Print the binary version and linked capabilities, then exit. |
| `-h`, `--help` | switch | | none | Print help. |
| | secret | | `ONMS_PASSWORD` | Password for a basic-auth context. Overrides the context's declared source. |
| | secret | | `ONMS_TOKEN` | Token for a bearer context. Overrides the context's declared source. |

For `--context`, `--url`, `--user` and `--read-only`, a flag beats its environment variable, which beats the active context.
`-o`, `-v` and `--insecure-tls` have no environment variable.

## Apply flags

| Flag | Behavior |
|---|---|
| `-f`, `--filename <FILE\|DIR\|GLOB>` | Required. Input file, directory or quoted glob; directories and globs read `.yaml`/`.yml` files. |
| `--dry-run` | Plan only; zero mutating HTTP. Classifies as a Read, so read-only contexts may run it. It still reads from the server. |
| `--diff` | Render each kind-bucket's diff to stderr (stdout stays clean for `-o json/yaml`). |
| `--continue-on-error` (alias `--keep-going`) | Keep applying after a failing document. Default is stop-on-error. |
| `-R`, `--recursive` | Recurse into subdirectories (off by default). |
| `--force` | Re-send a document that plans as unchanged. Only `SnmpConfig` honours it, to push a secret-only rotation. |

## Verb aliases

| Command | Alias |
|---|---|
| `event-source` | `evtsrc` |
| `event` | `evt` |
| `requisition` | `req` |
| `maintenance` | `maint` |
| `datacollection` | `dc` |
| `business-service` | `bs` |
| `threshold` | `thr` |
| `config` | `cfg` |

Both forms appear in `--help`.
