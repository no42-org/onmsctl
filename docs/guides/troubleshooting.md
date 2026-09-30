---
title: Troubleshooting
description: "Diagnose common onmsctl problems: wrong binary, config and context errors, auth failures, connection and TLS errors, and unexpected diffs."
---

Find the symptom, then apply the fix.
Every exit code is listed under [Exit codes](../reference/exit-codes.md).
Add **`-v`** / **`--verbose`** to any command to see the full error chain.

| Symptom | Cause | Fix |
|---|---|---|
| `onmsctl version` shows the wrong or an old binary. | A release binary in `/usr/local/bin` shadows a `cargo install` one in `~/.cargo/bin`, or the reverse. | Check **`which onmsctl`** and fix the `PATH` order. |
| `error: loading config from <path>` (exit `2`). | The config file does not exist at the default path, `--config`, or `$ONMSCTL_CONFIG`. | Create the file (see [Create the config file](../getting-started/configure-context.md#create-the-config-file)) or point **`--config`** at it. |
| `error: no context resolved` (exit `2`). | No `--context`, no `$ONMSCTL_CONTEXT`, and no `current-context` in the config. | Pass **`--context <name>`** or set `current-context`. |
| `error: context '<name>' not found in config` (exit `2`). | `--context` or `$ONMSCTL_CONTEXT` names a context the config does not define. | Run **`onmsctl config view`** and use a defined name. |
| Auth failures. | A set `$ONMS_PASSWORD` (basic auth) or `$ONMS_TOKEN` (bearer) overrides the context's declared secret, so a stale env var can silently win. | Confirm with **`onmsctl iam whoami`**. Unset the stale variable. See [Credentials](../getting-started/configure-context.md#credentials). |
| Commands reach the wrong server. | **`--url`** or `$ONMS_URL` overrides the active context. | Run **`onmsctl config view`** to see what is loaded. |
| `error: could not resolve host` (exit `4`). | Wrong host name in the server URL. | Check the URL and DNS. |
| `error: connection refused` (exit `5`). | Nothing listens on the host and port, or a firewall blocks it. | Check the URL and port. |
| `error: timed out connecting to / reading from` (exit `6`). | The server or network is too slow. | Retry, then check the server. |
| `error: TLS handshake failed` (exit `7`). | onmsctl trusts only its **bundled Mozilla root certificates**, not the OS trust store, so a private CA fails too. | See [TLS](../reference/tls.md). For a lab with a self-signed cert, **`--insecure-tls`** skips verification. Never use it in production. |
| `no YAML documents found in <file>` or `<dir> contains no .yaml / .yml files` (exit `2`). | `apply -f` got empty or comment-only input. | Point **`-f`** at the YAML you meant. Add **`-R`** if the files sit in subdirectories. |
| `requires --yes in non-interactive contexts` or `refusing in a non-interactive context` (exit `2`). | A confirm-guarded delete ran without a TTY on both stdin and stderr. | Add **`--yes`** (or `-y`). See the [breaking changes](migration.md#breaking-changes). |
| A write "did nothing". | A **`--dry-run`** writes nothing by design. | Re-run without `--dry-run` after review. |
| `error: active context '<name>' is read-only` (exit `12`). | `--read-only`, `$ONMSCTL_READ_ONLY`, or the context's `read-only: true` refused a Write verb. It is an error, not a silent no-op. | Neither the flag nor the variable can switch it off. Edit the context or switch contexts. See [Read-only contexts](../concepts/read-only-contexts.md). |
| Unexpected diff on re-apply. | Cosmetic reordering of set-like fields (categories, services) is normalized away, so a real diff means real drift. | Run **`apply --dry-run --diff`** and inspect the leaves. |
