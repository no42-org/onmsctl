# EventSource reference

Deep reference for `kind: EventSource` — the `event-source convert` finding
catalog, the YAML field semantics, and apply-time limitations. For the workflow,
see the [Quick Start](quickstart.md) and the [README](../README.md#event-configuration-kind-eventsource).

## `event-source convert`

`event-source convert` parses each event against the local `EventSource` schema
and emits findings on stderr. Example:

```
EC004  error    event missing required field: uei
  At:   bad.events.xml:14:5  (event[3])
  Fix:  Add the required uei to the event in the source XML.
  For the full rationale: onmsctl event-source convert --explain EC004
```

**Finding codes** (`EC001`–`EC008`, stable across releases; `--explain <code>`
for any rationale):

| Code | Severity | Meaning |
|------|----------|---------|
| EC001 | warning | Unmodeled direct-child element under `<event>` dropped on conversion. |
| EC002 | error   | Source has zero events. |
| EC003 | error   | Reserved `metadata.name`. |
| EC004 | error   | Event missing a required field. |
| EC005 | warning | Severity case normalized (e.g. `WARNING` → `Warning`). |
| EC006 | error   | Post-conversion validation failed (catch-all). |
| EC007 | error   | `alarm-type` outside the accepted set `{1, 2, 3}`. |
| EC008 | error   | Invalid `metadata.name` (disallowed characters). |

**Exit codes:** `0` clean, `1` warnings (YAML written), `2` blocking findings (no YAML).

**Flags:**

| Flag | Purpose |
|---|---|
| `--format json` | CI envelope with `output` path and `yaml` body. |
| `--max-bytes 64M` | Override the 16 MiB input cap. |
| `--max-findings 0` | Disable the 1000-finding `EC001` cap (set `<n>` for any other limit). |
| `--force` | Overwrite existing output. |
| `--explain <code>` | Print the full rationale for a finding code and exit. |

**Unmodeled elements.** `EC001` is the permanent forward-compatibility surface: any
direct-child element under `<event>` that the YAML schema doesn't model fires
`EC001` rather than silently losing data. The early modeling gaps (`<snmp>`,
`<parameter>`, `<forward>`, `<script>`, `<filters>`) are now first-class; remaining
unmodeled XSD elements (`<priority>`, `<autoaction>`, `<operaction>`, `<loggroup>`,
vendor extensions) keep firing `EC001` until modeled. For full fidelity today, keep
the XML alongside the YAML and use `event-source upload`. `EC001` is
**structural-only** — it does not detect attribute extensions on modeled elements or
enum-value drift on modeled fields.

## YAML field semantics

### `alarmType`

`spec.events[].alarmData.alarmType` accepts the three known states, in either
symbolic (Web UI) form or the integer it maps to:

| Symbolic | Integer |
|---|---|
| `raise` | `1` |
| `resolution` | `2` |
| `unresolvable` | `3` |

Symbolic input is case-insensitive on parse; canonical YAML output is always
lowercase. Anything else — unknown symbolic strings (`"problem"`, the alarmd Java
alias) or integers outside `{1, 2, 3}` — fails immediately. YAML inputs reject at
deserialize time; eventconf XML inputs produce an `EC007` finding (Error, exit 2).

### `snmp`

`spec.events[].snmp` mirrors the eventconf XSD's `<snmp>` element. Every sub-field is
optional. Practical numeric ranges are documented but NOT enforced — out-of-range
integers round-trip verbatim. String fields are rejected when empty/whitespace-only.

- `id` — enterprise OID; free string, no OID-format validation.
- `idtext` — vendor-supplied textual label.
- `version` — common values `v1` / `v2c` / `v3` (free string; variants like
  `v3-auth-priv` accepted verbatim).
- `generic` — `0..=6` per RFC 1157.
- `specific` — `>= 0`.
- `community` — typically `public`.

### `parameters`

`spec.events[].parameters` mirrors `<parameter name value expand/>` — *static*
per-event configuration eventd attaches to fired events. Each entry requires `name`
and `value`; `expand` is optional and controls whether eventd substitutes
`%parm[#N]%`-style placeholders at fire time. Document order is preserved.

This is **distinct** from `parmCollection` on a *fired* event instance (a runtime
field on the JSON wire, not modeled here). Similar names, different domains — do not
conflate them.

### `forwards` and `scripts`

`spec.events[].forwards` mirrors `<forward state mechanism>target</forward>` —
eventd's forwarding directives, validated against the XSD-closed sets:

- `state` ∈ `{on, off}`
- `mechanism` ∈ `{snmpudp, snmptcp, xmltcp, xmludp}`

Values outside these sets are rejected locally (otherwise Horizon returns a 400). An
empty `forwards: [{}]` entry is rejected — at least one of `state`/`mechanism`/`target`
must be set.

`spec.events[].scripts` mirrors `<script language>body</script>` — embedded
executable logic (typically BeanShell) eventd runs on event arrival. `language` is
REQUIRED; `body` is optional and preserved byte-for-byte (use YAML's `|` literal
block for multi-line bodies).

> **Security note.** Shipping executable code via `onmsctl apply` lowers the friction
> for deploying server-side code execution on Horizon. The threat surface already
> exists at the raw eventconf-XML upload path — modeling `<script>` in YAML adds no
> new authority — but ensure RBAC on eventconf write access reflects it: anyone who
> can upload an event source can run code on the Horizon JVM.

### `filters`

`spec.events[].filters` mirrors `<filters><filter eventparm pattern replacement/></filters>`.
Each entry is a regex-replacement rule eventd applies to a named event parameter at
fire time:

```
Pattern.compile(pattern).matcher(parmValue).replaceAll(replacement)
```

All three fields are required. `pattern` is Java regex; `replacement` supports
`$1`/`$2` backreferences. The YAML is flat (`filters:` directly on the event); the
`<filters>` wrapper materializes only on XML render.

**`<mask>` vs `<filters>`.** `<mask>` *selects* which events a source applies to
(SNMP PDU shape matching: `id` / `generic` / `specific` / varbind values, each an
OR-matched value list). `<filters>` operates *after* selection, rewriting parameter
values on the fired event. Two different layers.

## Apply-time limitations

(`onmsctl apply --help` for full text.)

| # | Limitation | Workaround |
|---|---|---|
| 1 | `description` not persisted server-side through `apply`. | Carry intent in the YAML and git review; the field round-trips locally. |
| 2 | Disabled-state `apply` has a bounded enabled-flap window. | `--verbose` warns when this runs. |
| 3 | `vendor` is filename-derived, not declared. | Encode as the prefix before the first `.` in `metadata.name`. |
| 4 | `fileOrder` is server-managed. | Deferred to a future `kind: EventConfMaster`. |
