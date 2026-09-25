---
title: Quick Start
description: Core concepts and a five-minute tour of onmsctl's declarative apply workflow.
---

A practical, end-to-end guide to installing, configuring, and using `onmsctl`, the command-line interface for OpenNMS Horizon.

`onmsctl` follows the kubectl pattern: one config file with named contexts, a single declarative `apply -f` mutation entrypoint, and read-only inspection verbs alongside it.
It is a single statically linked binary that bundles seven capabilities: **eventconf** (`event-source` / `event`), **provisioning** (`requisition`), **IAM** (`iam`), **SNMP config** (`snmp`), **maintenance** (`maintenance`), **data collection** (`datacollection`), and **BSM** (`business-service`).

> This is the fast path.
> For signature verification, the full `provision.pl` migration map, the EventSource schema reference, and server-compatibility notes, see the [README](https://github.com/no42-org/onmsctl/blob/main/README.md).

## Core concepts

**Declarative `apply -f` is the one mutation entrypoint.**
It peeks each YAML document's `kind` and routes it to the right handler.
There is no per-capability apply verb.
The recognized kinds:

| `kind` | `apiVersion` | Reconciles |
|---|---|---|
| `EventSource` | `eventconf.opennms.org/v1` | event configuration sources |
| `Requisition` | `provisioning.opennms.org/v1` | provisioning requisitions |
| `User` | `onmsctl.no42.org/v1alpha1` | Horizon users + roles |
| `SnmpConfig` | `snmp.opennms.org/v1` | SNMP agent + trap config (singleton) |
| `Maintenance` | `maintenance.opennms.org/v1` | scheduled-outage maintenance windows |
| `DataCollectionSource` | `datacollection.opennms.org/v1` | SNMP data-collection sources |
| `BusinessService` | `bsm.opennms.org/v1` | Business Service Monitoring (BSM) |

A single file may hold many `---`-separated documents, and a directory may mix any of these kinds.

**Plan → gate → execute.**
Every document is planned first.
If *any* document fails to plan (unknown `kind`, duplicate `metadata.name`, parse error), the whole apply aborts **before** any mutation.
Then documents execute in a fixed kind-precedence order, stopping at the first failure unless you pass `--continue-on-error`.

**`--dry-run` is always safe.**
It plans and prints but issues no mutating HTTP, so it is allowed even in a read-only context.
Pair it with `--diff` to see exactly what would change.

**Idempotent.**
Re-running the same input is the recovery path: an unchanged document reconciles to "no change" and skips the write.

**Read-only contexts.**
A context can set `read-only: true`, or you can pass `--read-only` (or set `ONMSCTL_READ_ONLY`).
Any write verb is then refused locally before any HTTP call (exit code `12`).
This is defense in depth on top of the server's own role checks.

## Five-minute tour

```sh
# 1. Who am I, and does my config work?
onmsctl iam whoami

# 2. What's on the server right now? (read-only)
onmsctl event-source list
onmsctl requisition list
onmsctl iam user list

# 3. Preview a change without touching the server.
onmsctl apply -f my-resource.yaml --dry-run --diff

# 4. Apply it for real.
onmsctl apply -f my-resource.yaml

# 5. Apply an entire directory of mixed resources.
onmsctl apply -f ./gitops/ --recursive
```

`--dry-run --diff` prints the rendered diff to **stderr** and a structured outcome to **stdout**, so you can review interactively or pipe the outcome to a tool.

## Next steps

Each `kind` has its own reference page:

- `EventSource`: event configuration sources.
- `Requisition`: provisioning requisitions.
- `User`: Horizon users and roles.
- `SnmpConfig`: SNMP agent and trap config (singleton).
- `Maintenance`: scheduled-outage maintenance windows.
- `DataCollectionSource`: SNMP data-collection sources.
- `BusinessService`: Business Service Monitoring (BSM).
