---
title: Quick Start
description: Core concepts and a five-minute tour of onmsctl's declarative apply workflow.
---

In five minutes you preview and apply a `Requisition` named `acme-prod` with three nodes.
You need an [installed](install.md) `onmsctl` and a [configured context](configure-context.md).

`onmsctl` follows the kubectl pattern: one config file with named contexts, a single declarative `apply -f` mutation entrypoint, and read-only inspection verbs alongside it.
It is a single statically linked binary that bundles nine capabilities: **eventconf** (`event-source` / `event`), **provisioning** (`requisition`), **IAM** (`iam`), **SNMP config** (`snmp`), **maintenance** (`maintenance`), **data collection** (`datacollection`), **BSM** (`business-service`), **thresholding** (`threshold`), and **GraphML topologies** (`graph`).

## Learn the core concepts

- **`apply -f` is the one mutation entrypoint.** It reads each YAML document's `kind` and routes it to that kind's handler. One file or directory may mix kinds. The [kind table](../concepts/declarative-apply.md) lists every kind and links its page.
- **Plan, gate, execute.** Every document is planned first. Any planning failure aborts the whole apply before a write. See [Declarative apply](../concepts/declarative-apply.md).
- **`--dry-run` is always safe.** It plans every document and issues no mutating request. Add `--diff` for field-level changes on the kinds that render one.
- **Read-only contexts** refuse every write locally with exit `12`. See [Read-only contexts](../concepts/read-only-contexts.md).

## Take the five-minute tour

1. Check that your config and credentials work.

   ```sh
   onmsctl iam whoami
   ```

   Expected output:

   ```text
   admin
   ```

   The output is the user name of the active context.

2. List the requisitions already on the server.

   ```sh
   onmsctl requisition list
   ```

   Expected output:

   ```text
   poc-scale
   selfmonitor
   ```

   Your server shows its own requisition names.

3. Download the example requisition.

   ```sh
   curl -fsSLO https://raw.githubusercontent.com/no42-org/onmsctl/main/examples/requisition-acme-prod.yaml
   ```

   It defines `acme-prod` with the nodes `web01`, `db01` and `cache01`, plus a pinned foreign-source definition.

4. Preview the change without touching the server.

   ```sh
   onmsctl apply -f requisition-acme-prod.yaml --dry-run
   ```

   Expected output:

   ```text
   +-------------+-----------+--------+---------+-----------------------+
   | kind        | name      | action | status  | message               |
   +====================================================================+
   | Requisition | acme-prod | create | Skipped | dry-run: would create |
   +-------------+-----------+--------+---------+-----------------------+
   ```

   **`--dry-run`** prints the planned action per document to **stdout** and sends no mutating request.
   `acme-prod` does not exist yet, so the action is `create`.

5. Apply the requisition for real.

   ```sh
   onmsctl apply -f requisition-acme-prod.yaml
   ```

   This step writes to the server.
   Run `onmsctl requisition list` again to see `acme-prod`.

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
- [`Graph`](../kinds/graph.mdx): GraphML topologies for the topology UI.

See also:

- [Migration](../guides/migration.md): the full `provision.pl` migration map.
- [Compatibility](../reference/compatibility.md): server-compatibility notes.
