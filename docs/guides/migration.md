---
title: Migration guide
description: Map imperative onmsctl verbs, provision.pl, and legacy users.xml workflows onto the declarative apply model.
---

Move to `onmsctl` from imperative tooling (`provision.pl`, hand-edited `users.xml`, the web UI) and from earlier `onmsctl` releases.
The old tools ran one mutation per command.
**`onmsctl apply -f`** takes the desired state in YAML, and the diff decides what to mutate.

## Replace removed imperative verbs

These imperative mutators no longer exist.
Declare the desired state in YAML and `apply` it:

| Removed verb(s) | Replacement |
|---|---|
| `event-source apply`, `event-source create`, `event-source enable`, `event-source disable` | Declare the source, its events, and enabled-state in a `kind: EventSource` document, then `onmsctl apply -f`. (`event-source upload` / `event-source download` still round-trip raw XML.) |
| `event add`, `event update`, `event delete`, `event enable`, `event disable` | Edit `spec.events[...]` in the owning `kind: EventSource` document, then `onmsctl apply -f`. (`event list` remains for inspection.) |
| `requisition apply` | `onmsctl apply -f` (kind `Requisition`). |
| `requisition node\|interface\|service\|category add\|set\|remove` | Edit `spec.nodes[...]` in the requisition YAML, then `onmsctl apply -f`. The read verbs remain for inspection: `node` and `interface` have `list` and `get`, `service` and `category` have `list`. |
| `iam apply`, `iam user create`, `iam user update`, `iam user role add`, `iam user role remove` | Declare a `kind: User` document (scalar fields + `roles` set + `passwordRef`), then `onmsctl apply -f`. `iam user set-password`, `iam user delete`, and the read verbs remain. |

## Replace `provision.pl` commands {#provisionpl-verb--onmsctl}

| `provision.pl` | `onmsctl` |
|---|---|
| `provision.pl requisition add <fs>` | `onmsctl apply -f <fs>.yaml` (a `kind: Requisition` document with an empty `nodes: []` payload) |
| `provision.pl requisition remove <fs>` | `onmsctl requisition delete <fs> --yes` (issues both `DELETE /rest/requisitions/<fs>` AND `DELETE /rest/requisitions/deployed/<fs>` in one call; idempotent on both, 404 on either snapshot is treated as success. **`--yes` is required in non-TTY contexts**; TTY contexts prompt interactively. Remove the local YAML separately) |
| `provision.pl requisition import <fs>` | `onmsctl requisition import <fs>` (PUT-only, no re-POST; add `--wait` to block until completion) |
| `provision.pl requisition list` | `onmsctl requisition list` (wraps `GET /rest/requisitionNames`; respects `-o` table / json / yaml) |
| `provision.pl node add <fs> <foreign-id> <node-label>` | Edit `spec.nodes[...]` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition node list / get` remain for inspection. |
| `provision.pl interface add <fs> <foreign-id> <ip>` | Edit the node's `interfaces` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition interface list / get` remain for inspection. |
| `provision.pl service add <fs> <foreign-id> <ip> <svc>` | Edit the interface's `services` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition service list` remains for inspection. |
| `provision.pl category add <fs> <foreign-id> <cat>` | Edit the node's `categories` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition category list` remains for inspection. |
| `provision.pl asset add <fs> <foreign-id> <name> <value>` | Edit the node's `spec.nodes[].assets` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. Post-import, takes-effect-immediately escape hatch: `onmsctl requisition asset set <db-id> <field> <value>` (sibling reads: `asset list / get`). **Misfit:** keyed by integer database node ID, not foreign-id; resolve it first with `curl -u <user> -H 'Accept: application/json' 'https://<host>/opennms/rest/nodes?foreignSource=<fs>&foreignId=<fid>'` and read the node's `id`. |

### Replace `provision.pl` shell automation

Run this recipe once per site:

1. **Convert** existing XML to YAML:
   ```sh
   onmsctl requisition convert \
     --from /opt/opennms/etc/imports/ \
     --foreign-sources-dir /opt/opennms/etc/foreign-sources/ \
     --out repo/yaml/
   ```
   Expected output (stderr, from a two-file sample):
   ```text
   PR004 info [cv/imports/clean-requisition.xml]: Requisition/acme-prod has no matching foreign-source XML; emitted YAML inherits Horizon's default-FS
   PR001 warning [cv/imports/unmodeled-elements-requisition.xml]: node 'web01': 'city' is not modeled in YAML (preserved as annotation — use asset for round-trip)
   ...
   PR002 warning [cv/foreign-sources/clean-foreign-source.xml]: orphan foreign-source XML 'clean-foreign-source'; no matching requisition
   ```
   Review each finding (`PR001`-`PR005`).
   Resolve it by editing the source XML (rare) or by accepting the documented data loss (most common).
   Each code's **`--explain`** text says which applies:
   ```sh
   onmsctl requisition convert --explain PR001
   ```
   Expected output:
   ```text
   PR001 — Unmodeled XML element or attribute.

   The migrator encountered an XML element or attribute it doesn't model. The input was preserved as best it could be (unknown elements may be dropped from the YAML output), but the YAML is no longer a lossless round-trip of the source XML.
   ...
   What to do: inspect the named element/attribute in the source file. If it's load-bearing, file an issue against onmsctl with the example.
   ```
   `convert` exits **`1`** when it reports warnings, even though the YAML is written, so a `set -e` script stops there.
   Exit `2` means a finding blocked the output.
2. **Commit** the YAML directory to git as the new source of truth.
3. **Rewrite** the existing `provision.pl` shell scripts as `onmsctl apply -f <fs>.yaml` invocations.
   The legacy step-by-step mutation pattern collapses to one apply per requisition.
4. **Schedule** the apply via CI or cron.
   `--dry-run --diff` is the review gate.
   The real apply runs only after review.

To pull requisitions edited in Horizon's UI back into git, use **`requisition export`**:

```sh
onmsctl requisition export --out repo/yaml/                      # every requisition, one file each
onmsctl requisition export --include-defaults --out repo/yaml/   # inline the default FS
```

With a name and no `--out`, export writes that requisition to stdout:

```sh
onmsctl requisition export poc-scale
```

Expected output:

```text
# yaml-language-server: $schema=https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/requisition.schema.json
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata:
  name: poc-scale
spec:
  foreignSource:
    scanInterval: 1d
  nodes: []
```

Without `--include-defaults`, the exported YAML omits `spec.foreignSource` when the server has no custom FS (the portable style).
With it, the default FS is inlined alongside a snapshot comment.
That inlined block is a point-in-time copy.
It does NOT stay in sync with Horizon's default after export.

## Replace `users.xml` edits {#legacy-usersxml--onmsctl}

| Pre-onmsctl | `onmsctl` |
|---|---|
| Hand-edit `$OPENNMS_HOME/etc/users.xml`, reload | `onmsctl apply -f users/` (one `kind: User` document per user; apply reconciles scalar fields + role set against the live server) |
| Add a user via the web UI | Add a `kind: User` document and `onmsctl apply -f` |
| Change a user's roles in the UI | Edit the document's `roles` set and `onmsctl apply -f` (roles reconcile as a set) |
| Rotate a password in the UI | `onmsctl iam user set-password <name> --password-stdin` |
| Remove a `<user>` element + reload | `onmsctl iam user delete <name> --yes` (idempotent; a 404 is a no-op) |

## Breaking changes

### Confirm `requisition delete` with `--yes` (since v0.1.1)

**`onmsctl requisition delete <fs>`** purges both pending and deployed snapshots in a single call.
That is a wider blast radius than any other Write verb, so it refuses to run without explicit acknowledgement:

- **Interactive (TTY) shells:** it shows the requisition name, node count and last-import timestamp (ISO-8601 UTC), then prompts for `yes` or `y` (case-insensitive).
  Any other input (`no`, an empty line, or Ctrl-D/EOF) cancels with exit **`2`**, so scripts can tell cancellation from success.
  If the requisition does not exist (the pre-confirm GET returns 404), it skips the prompt and the delete is a no-op.
- **Non-interactive runs (CI, scripted pipelines, redirected stdin or stderr):** **`--yes` is always required**, even when the requisition does not exist.
  Without it the verb refuses with an error pointing at `--yes`.
  The prompt fires only when both stdin and stderr are TTYs.

CI fix: add `--yes` (or `-y`) to existing invocations.
