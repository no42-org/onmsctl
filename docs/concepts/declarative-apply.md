---
title: Declarative apply
description: Reconcile YAML documents by kind with the onmsctl apply -f declarative entrypoint.
---

`onmsctl apply -f <file|dir|glob>` is the single declarative mutation entrypoint.
It peeks each YAML document's `kind` and routes it to the registered handler.
There is no per-capability apply verb.
Recognized kinds:

| `kind` | `apiVersion` | Reconciles |
|---|---|---|
| `User` | `onmsctl.no42.org/v1alpha1` | Horizon users + roles |
| `EventSource` | `eventconf.opennms.org/v1` | event configuration sources |
| `EventSourceOrder` | `eventconf.opennms.org/v1` | event-source evaluation order (singleton) |
| `ThresholdGroup` | `thresholding.opennms.org/v1` | threshold groups (thresholds and expressions) |
| `ThreshdPackage` | `thresholding.opennms.org/v1` | threshd packages (what is thresholded, with which group) |
| `SnmpConfig` | `snmp.opennms.org/v1` | SNMP agent + trap-daemon config (singleton) |
| `Requisition` | `provisioning.opennms.org/v1` | provisioning requisitions |
| `Maintenance` | `maintenance.opennms.org/v1` | scheduled-outage maintenance windows |
| `DataCollectionSource` | `datacollection.opennms.org/v1` | SNMP data-collection sources |
| `BusinessService` | `bsm.opennms.org/v1` | Business Service Monitoring (BSM) services + edges |

A single file may hold many `---`-separated documents, and a directory can mix all kinds.

**Plan → gate → execute.**
Every document is planned first.
If *any* fails to plan (unknown `kind`, duplicate `metadata.name`, parse error), the whole apply **aborts before any mutation**.
Once the gate passes, documents execute in a static precedence order so dependencies settle first:

```
User (100) → EventSource (200) → EventSourceOrder (210) → ThresholdGroup (220) → ThreshdPackage (230) → SnmpConfig (250) → Requisition (300) → Maintenance (350) → DataCollectionSource (375) → BusinessService (400)
```

Each document yields one `ApplyOutcome` row, rendered through `-o table|yaml|json`:

```text
+-------------+-----------+--------+---------+-----------------------+
| kind        | name      | action | status  | message               |
+====================================================================+
| Requisition | acme-prod | create | Skipped | dry-run: would create |
+-------------+-----------+--------+---------+-----------------------+
```

```sh
onmsctl apply -f users.yaml                       # single file
onmsctl apply -f ./desired-state/                 # directory (mixed kinds)
onmsctl apply -f ./desired-state/ -R              # recurse into subdirs
onmsctl apply -f 'sources/cisco-*.yaml'           # glob (quote it)
```

| Flag | Behavior |
|---|---|
| `--dry-run` | Plan only; zero mutating HTTP. Classifies as a Read, so `--read-only` contexts may run it. |
| `--diff` | Render each kind-bucket's diff to stderr (stdout stays clean for `-o json/yaml`). |
| `--continue-on-error` (alias `--keep-going`) | Keep applying after a failing document. Default is stop-on-error. |
| `-R` / `--recursive` | Recurse into subdirectories (off by default). |

**Exit codes:** `0` means every document applied or was unchanged.
`1` means a document failed or the plan gate refused the input.
Other failures keep their own codes: `2` for a usage or config error (including empty input), `4`-`9` for connection errors, `12` for a read-only refusal, `13`-`15` for the IAM safety gates.
The full table is under [Exit codes](../reference/exit-codes.md).
The imperative mutators that predated this model are gone: see the [migration guide](../guides/migration.md#removed-imperative-verbs--onmsctl-apply--f).
