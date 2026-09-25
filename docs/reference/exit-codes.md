---
title: Exit codes
description: Stable onmsctl exit codes, safe to branch on in scripts.
---

Exit codes are stable and safe to branch on:

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | HTTP non-success / partial-failure batch / post-upload state-sync failed |
| 2 | misuse / config error / generic internal |
| 4 | DNS resolution failure |
| 5 | connection refused |
| 6 | timeout |
| 7 | TLS handshake failed |
| 8 | redirect loop |
| 9 | unsupported authentication scheme |
| 10 | `--wait` timed out before the async operation completed |
| 11 | `--wait` observed the async operation fail server-side |
| 12 | write refused locally by a `read-only` context |
| 13 | `apply` refused: would empty a protected role (admin lockout) |
| 14 | `apply` refused: would strip the caller's own protected role / account (self-lockout) |
| 15 | `apply` refused: caller identity unavailable, so the self-lockout check can't run |
