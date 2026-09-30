---
title: Read-only contexts
description: How a read-only context refuses every write verb before any HTTP call.
---

Mark a context `read-only: true` (or pass `--read-only` / set `$ONMSCTL_READ_ONLY`) to refuse every Write verb locally, before any HTTP call.
This exits with code `12`.
The flag and `ONMSCTL_READ_ONLY` only turn read-only on.
`ONMSCTL_READ_ONLY` accepts `1`, `true`, `yes` or `on`; other values leave the context's setting in place.
Neither can turn off a context's `read-only: true`: edit the context or switch to another one.
`onmsctl apply --dry-run` classifies as a Read, so it runs in read-only contexts.

```yaml
contexts:
  - name: prod-audit
    read-only: true
    server:
      url: https://horizon.example.com/opennms
    auth:
      basic:
        username: auditor
```
