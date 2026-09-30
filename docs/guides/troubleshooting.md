---
title: Troubleshooting
description: "Diagnose common onmsctl problems: wrong binary, auth failures, TLS errors, and unexpected diffs."
---

- **`onmsctl version` shows the wrong/old binary.**
  Check `which onmsctl`; a release binary in `/usr/local/bin` may shadow a `cargo install` one in `~/.cargo/bin` (or vice versa).
- **Auth failures.**
  Confirm with `onmsctl iam whoami`.
  A set `$ONMS_PASSWORD` (basic auth) or `$ONMS_TOKEN` (bearer) overrides the context's declared secret.
  A stale env var can silently override the config.
- **Wrong server.**
  `--url`/`$ONMS_URL` override the active context.
  Run `onmsctl config view` to see what's actually loaded.
- **TLS handshake failed (exit 7).**
  onmsctl trusts only its bundled Mozilla root certificates, not the OS trust store, so a private CA fails too.
  For a lab with a self-signed cert, `--insecure-tls` skips verification (never in production).
- **A write "did nothing".**
  A `--dry-run` writes nothing by design.
  A read-only refusal is an error with exit `12`, not a silent no-op.
  It comes from `--read-only`, `$ONMSCTL_READ_ONLY`, or the context's `read-only: true`; neither the flag nor the variable can switch it off, so edit the context or switch contexts.
- **Unexpected diff on re-apply.**
  Run `apply --dry-run --diff` and inspect the leaves; cosmetic reordering of set-like fields (categories, services) is normalized away, so a real diff means real drift.
- **See the full error chain.**
  Add `-v`.
