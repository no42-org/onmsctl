# onmsctl examples

Declarative manifests for `onmsctl apply -f <file>`. Each file is a valid
document for one `kind`; apply them against a configured context (add `--dry-run`
to preview without writing). Filenames are prefixed with their kind.

| File | Kind | Demonstrates | Docs |
|------|------|--------------|------|
| [`requisition-acme-prod.yaml`](requisition-acme-prod.yaml) | `Requisition` | A provisioning requisition: nodes, interfaces, services, categories, assets. | [docs](https://onmsctl.no42.org/kinds/requisition) |
| [`iam-user.yaml`](iam-user.yaml) | `User` | An IAM user with roles. | [docs](https://onmsctl.no42.org/kinds/user) |
| [`snmp-config.yaml`](snmp-config.yaml) | `SnmpConfig` | The singleton SNMP config: defaults, profiles, definitions. | [docs](https://onmsctl.no42.org/kinds/snmp-config) |
| [`maintenance.yaml`](maintenance.yaml) | `Maintenance` | A scheduled-outage window with per-daemon suppression. | [docs](https://onmsctl.no42.org/kinds/maintenance) |
| [`datacollection-source.yaml`](datacollection-source.yaml) | `DataCollectionSource` | A datacollection-group: groups, resource types, system defs, profiles, inline `profileSpec`. | [docs](https://onmsctl.no42.org/kinds/datacollection-source) |
| [`business-service.yaml`](business-service.yaml) | `BusinessService` | A BSM service with all four edge types (child / ip-service / application / reduction-key), map/reduce functions, node-by-label, and `{{nodeId}}` templating. | [docs](https://onmsctl.no42.org/kinds/business-service) |
| [`event-source-minimal.yaml`](event-source-minimal.yaml) | `EventSource` | The smallest valid EventSource document. | [docs](https://onmsctl.no42.org/kinds/event-source) |
| [`event-source-full.yaml`](event-source-full.yaml) | `EventSource` | Every nested type the EventSource model supports (mask, alarmData, varbinds, snmp, forwards, scripts, filters, …). | [docs](https://onmsctl.no42.org/kinds/event-source) |
| [`event-source-severities.yaml`](event-source-severities.yaml) | `EventSource` | The seven case-sensitive severity levels. | [docs](https://onmsctl.no42.org/kinds/event-source) |
| [`event-source-disabled.yaml`](event-source-disabled.yaml) | `EventSource` | `enabled: false` (applied via upload-then-disable; brief enabled-flap — see `apply --help`). | [docs](https://onmsctl.no42.org/kinds/event-source) |

The `event-source-*` fixtures are also checked by a unit test
(`published_examples_parse_against_the_schema`) so they cannot silently drift out
of sync with the EventSource model.
