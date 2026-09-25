---
title: Output formats
description: Choose table, YAML, or JSON output for scripting onmsctl.
---

Every `list`/`get` accepts `-o table` (default), `-o yaml`, or `-o json`.

Pick a machine-readable format for scripting:

```sh
onmsctl event-source list -o json | jq .
onmsctl requisition export acme-prod -o yaml
```
