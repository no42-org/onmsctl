---
title: Verb aliases and read-only contexts
description: Short verb aliases and how a read-only context refuses every write verb before any HTTP call.
---

## Verb aliases

Short aliases: `event-source`→`evtsrc`, `event`→`evt`, `requisition`→`req`, `config`→`cfg`, `maintenance`→`maint`, `datacollection`→`dc`.
Both forms appear in `--help`.

## Read-only contexts

Mark a context `read-only: true` (or pass `--read-only` / set `$ONMSCTL_READ_ONLY`) to refuse every Write verb locally, before any HTTP call.
This exits with code `12`.
Precedence is **flag > env > context > default false**; the flag is one-way.
`--dry-run apply` classifies as a Read, so it runs in read-only contexts.
