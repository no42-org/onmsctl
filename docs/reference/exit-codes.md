---
title: Exit codes
description: Stable onmsctl exit codes, safe to branch on in scripts.
---

Every command except the conversion commands below uses these codes.
Changing a code requires a spec amendment.

| Code | Meaning | Probable cause | Recovery |
|---|---|---|---|
| 0 | success | | |
| 1 | request or document failed | HTTP non-success, a failed document in a batch, a plan-gate refusal in `apply` (unknown `kind`, duplicate `metadata.name`, a kind's own gate), a post-upload state-sync failure, user not found | Read the error message; re-run after fixing the input or the server state. |
| 2 | misuse, config or internal error | Invalid flags, config file errors, no or unknown context, no password/token source, empty `apply` input, I/O, YAML/JSON parse errors | Fix the command line or the config file. |
| 4 | DNS resolution failure | Wrong host name in the server URL | Check the URL and DNS. |
| 5 | connection refused | Nothing listening, a firewall, or an untrusted TLS certificate (currently reported as a refused connection, see [TLS](tls.md)) | Check the URL and port; test the certificate with `curl -v`. |
| 6 | timeout | Server or network too slow | Retry; check the server. |
| 7 | TLS handshake failed | TLS errors outside the connect phase | See [TLS](tls.md). |
| 8 | redirect loop | Wrong base URL or proxy config | Point the URL at `/opennms`. |
| 9 | unsupported authentication scheme | The server asks for an auth scheme other than Basic or Bearer | Use a basic or bearer context. |
| 10 | `--wait` timed out | The async operation outlasted `--timeout` (default `30m`) | Raise `--timeout` or check the operation's progress. |
| 11 | `--wait` observed a server-side failure | The async operation failed on the server | Read the server-side error. |
| 12 | write refused by a read-only context | `--read-only`, `ONMSCTL_READ_ONLY`, or the context's `read-only: true` | Switch to a writable context. |
| 13 | IAM `User` apply refused: admin lockout | The apply would remove the last holder of a protected role | Keep a holder, or set `iam.allow-admin-lockout: true` in the context. |
| 14 | IAM `User` apply refused: self-lockout | The apply would strip the caller's own protected role or account | Apply with a different context. |
| 15 | IAM `User` apply refused: caller unknown | The self-lockout check could not identify the caller | Use a basic-auth context, or preview with `--dry-run`. |

## Conversion commands

`event-source convert`, `event-source export`, `event-source download --format yaml` and `requisition convert` report conversion results with their own codes:

| Code | Meaning |
|---|---|
| 0 | clean, or info-only findings |
| 1 | warnings; the YAML is still written |
| 2 | blocking findings; no YAML written (also usage errors for `requisition convert`) |
| 3 | usage error before conversion ran (`event-source convert` only) |

A script running under `set -e` stops on `1` even though the output was written.
