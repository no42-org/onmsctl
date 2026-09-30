---
title: Server compatibility
description: Supported OpenNMS Horizon versions and known eventconf quirks onmsctl works around.
---

onmsctl supports OpenNMS Horizon **35 and later**.
Some kinds need a newer build, listed below.

| Server | Status |
|---|---|
| OpenNMS Horizon **35+** | Primary target (EventConf REST reproducible on 35.0.5 / 36.0.0). |

## Minimum server versions

onmsctl checks for newer endpoints and fails with a version message instead of a bare error.

| Kind | Server requirement | When onmsctl checks |
|---|---|---|
| `EventSource`, `Requisition`, `User`, `Maintenance`, `BusinessService` | Any supported Horizon | No version check |
| `EventSourceOrder` | A server that reorders sources through an `eventconf.xml` upload part | After the write: a server that ignores the order fails the document with `server did not apply the requested order` |
| `SnmpConfig` | Any supported Horizon; the `spec.trapd` block needs the Trapd REST API (NMS-19128, `37.x`/`develop`) | On the trapd write only; `--dry-run` does not detect it |
| `DataCollectionSource` | A Horizon build with the DB-backed data-collection subsystem (absent from released Horizon ≤ 37.0.0) | Before every `datacollection` command and apply |
| `ThresholdGroup`, `ThreshdPackage` | Horizon 37.0.0 (NMS-19837) | Before every `threshold` command and apply |

## Known server quirks

These eventconf quirks are tracked upstream.
onmsctl works around the load-bearing ones client-side.

| Symptom | Cause | Fix |
|---|---|---|
| `event-source list` prints empty despite existing sources (observed on 35.0.5 / 36.0.0). | Server-side listing bug ([NMS-19810](https://opennms.atlassian.net/browse/NMS-19810)). | Use **`event-source names-and-ids`**. |
| An upload fails or stores the source under an unexpected name. | `POST /eventconf/upload` requires the multipart field name to be literally `upload` ([NMS-19813](https://opennms.atlassian.net/browse/NMS-19813)). It derives the source name by stripping only the final extension. | None needed. onmsctl sends the `upload` field and uploads **`{metadata.name}.xml`**, so the stored name equals `metadata.name`. |
