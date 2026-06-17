# onmsctl examples

Declarative manifests for `onmsctl apply -f <file>`. Each file is a valid
document for one `kind`; apply them against a configured context (add `--dry-run`
to preview without writing). Filenames are prefixed with their kind.

| File | Kind | Demonstrates | Docs |
|------|------|--------------|------|
| [`requisition-acme-prod.yaml`](requisition-acme-prod.yaml) | `Requisition` | A provisioning requisition: nodes, interfaces, services, categories, assets. | [README](../README.md#provisioning-kind-requisition) · [quickstart §7](../docs/quickstart.md) |
| [`iam-user.yaml`](iam-user.yaml) | `User` | An IAM user with roles. | [README](../README.md#users-and-roles-kind-user) · [quickstart §8](../docs/quickstart.md) |
| [`snmp-config.yaml`](snmp-config.yaml) | `SnmpConfig` | The singleton SNMP config: defaults, profiles, definitions. | [README](../README.md#snmp-configuration-kind-snmpconfig) · [quickstart §9](../docs/quickstart.md) |
| [`maintenance.yaml`](maintenance.yaml) | `Maintenance` | A scheduled-outage window with per-daemon suppression. | [README](../README.md#maintenance-windows-kind-maintenance) |
| [`datacollection-source.yaml`](datacollection-source.yaml) | `DataCollectionSource` | A datacollection-group: groups, resource types, system defs, profiles, inline `profileSpec`. | [README](../README.md#snmp-data-collection-kind-datacollectionsource) · [quickstart §9b](../docs/quickstart.md) |
| [`business-service.yaml`](business-service.yaml) | `BusinessService` | A BSM service with all four edge types (child / ip-service / application / reduction-key), map/reduce functions, node-by-label, and `{{nodeId}}` templating. | [README](../README.md#business-services-kind-businessservice) |
| [`event-source-minimal.yaml`](event-source-minimal.yaml) | `EventSource` | The smallest valid EventSource document. | [README](../README.md#event-configuration-kind-eventsource) |
| [`event-source-full.yaml`](event-source-full.yaml) | `EventSource` | Every nested type the EventSource model supports (mask, alarmData, varbinds, snmp, forwards, scripts, filters, …). | [README](../README.md#event-configuration-kind-eventsource) |
| [`event-source-severities.yaml`](event-source-severities.yaml) | `EventSource` | The seven case-sensitive severity levels. | [README](../README.md#event-configuration-kind-eventsource) |
| [`event-source-disabled.yaml`](event-source-disabled.yaml) | `EventSource` | `enabled: false` (applied via upload-then-disable; brief enabled-flap — see `apply --help`). | [README](../README.md#event-configuration-kind-eventsource) |

The `event-source-*` fixtures are also checked by a unit test
(`published_examples_parse_against_the_schema`) so they cannot silently drift out
of sync with the EventSource model.
