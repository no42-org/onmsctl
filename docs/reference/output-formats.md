---
title: Output formats
description: Choose table, YAML, or JSON output for scripting onmsctl.
---

`-o` is a global flag.
It defaults to **`table`**.

| Format | Shape | Use for |
|---|---|---|
| `table` | Bordered text, one row per item | Reading in a terminal |
| `json` | Pretty-printed JSON of the list or object | Scripts and `jq` |
| `yaml` | YAML of the list or object | Reading structured output, diffing |

## Print JSON

```sh
onmsctl event-source names -o json
```

```text
[
  "opennms.alarm.events",
  "opennms.catch-all.events",
  "opennms.linkd.events",
  ...
  "opennms.discovery.events",
  "opennms.ticketd.events"
]
```

Pipe the JSON into `jq`:

```sh
onmsctl event-source list -o json | jq .
```

```text
[
  {
    "id": 23,
    "name": "opennms.catch-all.events",
    "vendor": "opennms",
    "description": "",
    "fileOrder": 1,
    "eventCount": 4,
    "enabled": true,
    "createdTime": 1790699343537,
    "lastModified": 1790699343537,
    "uploadedBy": "system-migration"
  },
  ...
]
```

## Commands that override `-o`

Commands whose output is a document ignore part or all of `-o`:

| Command | Output |
|---|---|
| `event-source export` | Always YAML |
| `requisition export` | YAML for `table` and `yaml`; JSON only for `-o json` |
| `threshold group get`, `threshold package get`, `threshold export` | Always YAML |

With `-o json`, `requisition export` wraps each requisition's YAML in a JSON object:

```sh
onmsctl requisition export poc-scale -o json
```

```text
[
  {
    "foreign_source": "poc-scale",
    "yaml": "# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/requisition.schema.json\napiVersion: provisioning.opennms.org/v1\nkind: Requisition\nmetadata:\n  name: poc-scale\nspec:\n  foreignSource:\n    scanInterval: 1d\n  nodes: []\n",
    "default_fs_inlined": false
  }
]
```
