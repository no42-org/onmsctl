---
title: Server compatibility
description: Supported OpenNMS Horizon versions and known eventconf quirks onmsctl works around.
---

| Server | Status |
|---|---|
| OpenNMS Horizon **35+** | Primary target (EventConf REST reproducible on 35.0.5 / 36.0.0). |

Some kinds need newer server builds.
onmsctl checks for those endpoints and fails with a version message instead of a bare error:

| Kind | Server requirement | When onmsctl checks |
|---|---|---|
| `EventSource`, `Requisition`, `User`, `Maintenance`, `BusinessService` | Any supported Horizon | No version check |
| `EventSourceOrder` | A server that reorders sources through an `eventconf.xml` upload part | After the write: a server that ignores the order fails the document with `server did not apply the requested order` |
| `SnmpConfig` | Any supported Horizon; the `spec.trapd` block needs the Trapd REST API (NMS-19128, `37.x`/`develop`) | On the trapd write only; `--dry-run` does not detect it |
| `DataCollectionSource` | A Horizon build with the DB-backed data-collection subsystem (absent from released Horizon ≤ 37.0.0) | Before every `datacollection` command and apply |
| `ThresholdGroup`, `ThreshdPackage` | Horizon 37.0.0 (NMS-19837) | Before every `threshold` command and apply |

**Known eventconf quirks** (Horizon 35.0.5 / 36.0.0, tracked upstream; onmsctl works around the load-bearing ones client-side):

- `event-source list` can print empty despite existing sources ([NMS-19810](https://opennms.atlassian.net/browse/NMS-19810)).
  Use `event-source names-and-ids`; `apply --diff` may show a whole document as "added" rather than a true delta, though the upload still succeeds.
- `POST /eventconf/upload` requires the multipart field name to be literally `upload` ([NMS-19813](https://opennms.atlassian.net/browse/NMS-19813)) and derives the source name by stripping only the final extension.
  onmsctl handles both, and uploads `{metadata.name}.xml` so the stored name equals `metadata.name`.
