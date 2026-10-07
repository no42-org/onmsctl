---
title: Editor integration
description: Validate onmsctl YAML in your editor with the published JSON Schemas.
---

JSON Schemas (draft 2020-12) live under [`schemas/`](https://github.com/no42-org/onmsctl/tree/main/schemas), one per kind.
Any editor with [`yaml-language-server`](https://github.com/redhat-developer/yaml-language-server) can validate against them.

## Validate YAML in your editor

1. Add the modeline as the **first line** of each YAML file:

   ```yaml
   # yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/event-source.schema.json
   ```

2. Swap the filename for the document's `kind`.
   Pick it from the [schema files](#schema-files) table.

3. Optional: pin a release tag instead of `main` for stability.

   ```text
   https://raw.githubusercontent.com/no42-org/onmsctl/<tag>/schemas/<file>
   ```

   The tag must contain the schema.
   Kinds added after that release have no schema file at that tag.
   To validate against a local clone instead, point the modeline at the file on disk:

   ```yaml
   # yaml-language-server: $schema=<path-to-clone>/schemas/<file>
   ```

`requisition export` writes this modeline into its YAML output for you.

## Schema files

| `kind` | Schema file |
|---|---|
| `EventSource` | `event-source.schema.json` |
| `EventSourceOrder` | `event-source-order.schema.json` |
| `ThresholdGroup` | `threshold-group.schema.json` |
| `ThreshdPackage` | `threshd-package.schema.json` |
| `Requisition` | `requisition.schema.json` |
| `User` | `iam-user.schema.json` |
| `SnmpConfig` | `snmp-config.schema.json` |
| `Maintenance` | `maintenance.schema.json` |
| `DataCollectionSource` | `datacollection.schema.json` |
| `BusinessService` | `business-service.schema.json` |
| `Graph` | `graph.schema.json` |

Regenerate the schemas with `make schema`.
CI fails if a committed schema lags its Rust types.

### Tell ordered lists from sets

The requisition schema annotates list fields with **`x-onmsctl-list-kind: ordered|set`**.
Diff tooling uses it to tell ordered sequences (`detectors`, `policies`) from sets (`categories`, `services`).
