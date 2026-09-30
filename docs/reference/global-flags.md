---
title: Global flags and environment variables
description: Flags and environment variables that work across onmsctl commands, and their override precedence.
---

These work on every command:

| Flag | Env | Purpose |
|---|---|---|
| `--config <path>` | `ONMSCTL_CONFIG` | Config file path |
| `--context <name>` | `ONMSCTL_CONTEXT` | Active context |
| `--url <url>` | `ONMS_URL` | Server URL override |
| `--user <name>` | `ONMS_USER` | Basic-auth username override |
| `--read-only` | `ONMSCTL_READ_ONLY` | Refuse write verbs locally. Both only turn read-only on; the variable accepts `1`, `true`, `yes`, `on` |
| `-o, --output <fmt>` | | `table` (default), `yaml`, or `json` |
| `--insecure-tls` | | Skip TLS verification (avoid in prod) |
| `-v, --verbose` | | Full error chains + extra diagnostics |
| `-V, --version` | | Print the binary version and linked capabilities, then exit |
| `-h, --help` | | Print help |
| | `ONMS_PASSWORD` | Password for a basic-auth context; overrides the context's declared source |
| | `ONMS_TOKEN` | Token for a bearer context; overrides the context's declared source |

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
