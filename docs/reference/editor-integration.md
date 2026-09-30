---
title: Editor integration
description: Validate onmsctl YAML in your editor with the published JSON Schemas.
---

JSON Schemas (draft 2020-12) live under [`schemas/`](https://github.com/no42-org/onmsctl/tree/main/schemas), one per kind.
Add a modeline to the top of your YAML for in-editor validation via [`yaml-language-server`](https://github.com/redhat-developer/yaml-language-server):

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/event-source.schema.json
```

Swap the filename for the document's kind:

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

Pin a release tag for stability, or reference a local clone (`./schemas/<name>.schema.json`).
Regenerate with `make schema` (CI fails if a committed artifact lags).
The requisition schema annotates list fields with `x-onmsctl-list-kind: ordered|set` so diff tooling distinguishes ordered sequences (`detectors`, `policies`) from sets (`categories`, `services`).
