---
title: TLS
description: Disable TLS certificate verification for lab or self-signed environments.
---

onmsctl verifies server certificates against its bundled Mozilla root certificates (`webpki-roots`).
It does not read the OS trust store and has no option for a custom CA, so a server signed by a private CA fails verification even when that CA is trusted by the OS.
An untrusted certificate fails with `error: TLS handshake failed` and exit `7`.

`server.insecure-skip-tls-verify: true` (or `--insecure-tls`) disables certificate verification and emits a per-request stderr warning.
There is no environment variable for it, and `--insecure-tls` cannot turn verification back on for a context that disables it.
Keep it off in production.
