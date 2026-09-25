---
title: TLS
description: Disable TLS certificate verification for lab or self-signed environments.
---

`server.insecure-skip-tls-verify: true` (or `--insecure-tls`) disables certificate
verification and emits a per-request stderr warning.
Keep it off in production.
