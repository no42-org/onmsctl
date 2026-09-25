---
title: Server compatibility
description: Supported OpenNMS Horizon versions and known eventconf quirks onmsctl works around.
---

| Server | Status |
|---|---|
| OpenNMS Horizon **35+** | Primary target (EventConf REST reproducible on 35.0.5 / 36.0.0). |

Some capabilities need newer builds: the SNMP **Trapd** block (NMS-19128, `37.x`/`develop`) and **data collection** (absent from released Horizon ≤ 37.0.0).
Both gate cleanly with a clear version message on older servers.

**Known eventconf quirks** (Horizon 35.0.5 / 36.0.0, tracked upstream; onmsctl works around the load-bearing ones client-side):

- `event-source list` can print empty despite existing sources ([NMS-19810](https://opennms.atlassian.net/browse/NMS-19810)).
  Use `event-source names-and-ids`; `apply --diff` may show a whole document as "added" rather than a true delta, though the upload still succeeds.
- `POST /eventconf/upload` requires the multipart field name to be literally `upload` ([NMS-19813](https://opennms.atlassian.net/browse/NMS-19813)) and derives the source name by stripping only the final extension.
  onmsctl handles both, and uploads `{metadata.name}.xml` so the stored name equals `metadata.name`.
