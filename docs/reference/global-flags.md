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

`apply` adds `-f`/`--filename`, `--dry-run`, `--diff`, `--continue-on-error` (alias `--keep-going`), `-R`/`--recursive`, and `--force` (honoured by `SnmpConfig` only).

Override precedence, highest wins:

```
flags  >  environment  >  active context  >  built-in default
```

Top-level verbs have short aliases; see [Verb aliases](../concepts/read-only-contexts.md#verb-aliases).
