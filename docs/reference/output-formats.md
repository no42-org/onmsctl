---
title: Output formats
description: Choose table, YAML, or JSON output for scripting onmsctl.
---

`-o` is a global flag: `-o table` (default), `-o yaml`, or `-o json`.

```sh
onmsctl event-source list -o json | jq .
onmsctl requisition export acme-prod -o json
```

Commands whose output is a document ignore part or all of `-o`:

| Command | Output |
|---|---|
| `event-source export` | Always YAML |
| `requisition export` | YAML for `table` and `yaml`; JSON only for `-o json` |
| `threshold group get`, `threshold package get`, `threshold export` | Always YAML |
