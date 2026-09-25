---
title: Troubleshooting
description: "Diagnose common onmsctl problems: wrong binary, auth failures, TLS errors, and unexpected diffs."
---

- **`onmsctl version` shows the wrong/old binary.**
  Check `which onmsctl`; a release binary in `/usr/local/bin` may shadow a `cargo install` one in `~/.cargo/bin` (or vice versa).
- **Auth failures.**
  Confirm with `onmsctl iam whoami`.
  Remember the resolution order: `$ONMS_PASSWORD`/`$ONMS_TOKEN` beats keyring beats file beats inline.
  A stale env var can silently override the config.
- **Wrong server.**
  `--url`/`$ONMS_URL` override the active context.
  Run `onmsctl config view` to see what's actually loaded.
- **TLS handshake failed (exit 7).**
  For a lab with a self-signed cert, `--insecure-tls` skips verification (never in production).
- **A write "did nothing".**
  You're likely in a read-only context (exit `12`) or it was a `--dry-run`.
  Drop `--dry-run` / `--read-only`.
- **Unexpected diff on re-apply.**
  Run `apply --dry-run --diff` and inspect the leaves; cosmetic reordering of set-like fields (categories, services) is normalized away, so a real diff means real drift.
- **See the full error chain.**
  Add `-v`.
