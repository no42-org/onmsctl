---
title: Quick Start
description: Core concepts and a five-minute tour of onmsctl's declarative apply workflow.
---

This page covers the core concepts of `onmsctl`, the command-line interface for OpenNMS Horizon, and a five-minute tour.
It assumes you have [installed](install.md) `onmsctl` and [configured a context](configure-context.md).

`onmsctl` follows the kubectl pattern: one config file with named contexts, a single declarative `apply -f` mutation entrypoint, and read-only inspection verbs alongside it.
It is a single statically linked binary that bundles eight capabilities: **eventconf** (`event-source` / `event`), **provisioning** (`requisition`), **IAM** (`iam`), **SNMP config** (`snmp`), **maintenance** (`maintenance`), **data collection** (`datacollection`), **BSM** (`business-service`), and **thresholding** (`threshold`).

> This is the fast path.
> For signature verification, see [Install](install.md).
> For the full `provision.pl` migration map, see [Migration](../guides/migration.md).
> For the EventSource schema reference, see [EventSource](../kinds/event-source.mdx).
> For server-compatibility notes, see [Compatibility](../reference/compatibility.md).

## Core concepts

- **`apply -f` is the one mutation entrypoint.** It reads each YAML document's `kind` and routes it to that kind's handler; one file or directory may mix kinds. The [kind table](../concepts/declarative-apply.md) lists every kind and links its page.
- **Plan, gate, execute.** Every document is planned first, and any planning failure aborts the whole apply before a write. See [Declarative apply](../concepts/declarative-apply.md).
- **`--dry-run --diff` is always safe.** It shows what would change and issues no mutating request.
- **Read-only contexts** refuse every write locally with exit `12`. See [Read-only contexts](../concepts/read-only-contexts.md).

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

- [`EventSource`](../kinds/event-source.mdx): event configuration sources.
- [`EventSourceOrder`](../kinds/event-source-order.mdx): event-source evaluation order (singleton).
- [`ThresholdGroup`](../kinds/threshold-group.mdx): threshold groups.
- [`ThreshdPackage`](../kinds/threshd-package.mdx): threshd packages.
- [`Requisition`](../kinds/requisition.mdx): provisioning requisitions.
- [`User`](../kinds/user.mdx): Horizon users and roles.
- [`SnmpConfig`](../kinds/snmp-config.mdx): SNMP agent and trap config (singleton).
- [`Maintenance`](../kinds/maintenance.mdx): scheduled-outage maintenance windows.
- [`DataCollectionSource`](../kinds/datacollection-source.mdx): SNMP data-collection sources.
- [`BusinessService`](../kinds/business-service.mdx): Business Service Monitoring (BSM).
