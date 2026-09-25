---
title: Global flags and environment variables
description: Flags and environment variables that work across onmsctl commands, and their override precedence.
---

These work on (almost) every command:

| Flag | Env | Purpose |
|---|---|---|
| `--config <path>` | `ONMSCTL_CONFIG` | Config file path |
| `--context <name>` | `ONMSCTL_CONTEXT` | Active context |
| `--url <url>` | `ONMS_URL` | Server URL override |
| `--user <name>` | `ONMS_USER` | Basic-auth username override |
| `--read-only` | `ONMSCTL_READ_ONLY` | Refuse write verbs locally |
| `-o, --output <fmt>` | | `table` (default), `yaml`, or `json` |
| `--insecure-tls` | | Skip TLS verification (avoid in prod) |
| `-v, --verbose` | | Full error chains + extra diagnostics |
| | `ONMS_PASSWORD` / `ONMS_TOKEN` | Highest-priority credential source |

`apply` adds `-f`/`--filename`, `--dry-run`, `--diff`, `--continue-on-error` (alias `--keep-going`), and `-R`/`--recursive`.

Override precedence, highest wins:

```
flags  >  environment  >  active context  >  built-in default
```

Top-level verbs have short aliases; see [Verb aliases](../concepts/read-only-contexts.md#verb-aliases).
