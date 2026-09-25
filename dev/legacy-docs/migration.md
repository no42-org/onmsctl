# Migration guide

Moving to `onmsctl` from imperative tooling (`provision.pl`, hand-edited
`users.xml`, the web UI) and from earlier `onmsctl` releases. The throughline:
where the old tools ran one mutation per command, `onmsctl apply -f` ships the
desired state in YAML and lets the diff decide what to mutate.

## Removed imperative verbs → `onmsctl apply -f`

These imperative mutators no longer exist. Declare the desired state in YAML and
`apply` it:

| Removed verb(s) | Replacement |
|---|---|
| `event-source apply`, `event-source create`, `event-source enable`, `event-source disable` | Declare the source, its events, and enabled-state in a `kind: EventSource` document, then `onmsctl apply -f`. (`event-source upload` / `event-source download` still round-trip raw XML.) |
| `event add`, `event update`, `event delete`, `event enable`, `event disable` | Edit `spec.events[...]` in the owning `kind: EventSource` document, then `onmsctl apply -f`. (`event list` remains for inspection.) |
| `requisition apply` | `onmsctl apply -f` (kind `Requisition`). |
| `requisition node\|interface\|service\|category add\|set\|remove` | Edit `spec.nodes[...]` in the requisition YAML, then `onmsctl apply -f`. The matching `… list` / `get` sub-resource verbs remain for inspection. |
| `iam apply`, `iam user create`, `iam user update`, `iam user role add`, `iam user role remove` | Declare a `kind: User` document (scalar fields + `roles` set + `passwordRef`), then `onmsctl apply -f`. `iam user set-password`, `iam user delete`, and the read verbs remain. |

## `provision.pl <verb>` → `onmsctl`

| `provision.pl` | `onmsctl` |
|---|---|
| `provision.pl requisition add <fs>` | `onmsctl apply -f <fs>.yaml` (a `kind: Requisition` document with an empty `nodes: []` payload) |
| `provision.pl requisition remove <fs>` | `onmsctl requisition delete <fs> --yes` (issues both `DELETE /rest/requisitions/<fs>` AND `DELETE /rest/requisitions/deployed/<fs>` in one call; idempotent on both — 404 on either snapshot is treated as success. **`--yes` is required in non-TTY contexts**; TTY contexts prompt interactively. Remove the local YAML separately) |
| `provision.pl requisition import <fs>` | `onmsctl requisition import <fs>` (PUT-only, no re-POST; add `--wait` to block until completion) |
| `provision.pl requisition list` | `onmsctl requisition list` (wraps `GET /rest/requisitionNames`; respects `-o` table / json / yaml) |
| `provision.pl node add <fs> <foreign-id> <node-label>` | Edit `spec.nodes[...]` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition node list / get` remain for inspection. |
| `provision.pl interface add <fs> <foreign-id> <ip>` | Edit the node's `interfaces` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition interface list / get` remain for inspection. |
| `provision.pl service add <fs> <foreign-id> <ip> <svc>` | Edit the interface's `services` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition service list` remains for inspection. |
| `provision.pl category add <fs> <foreign-id> <cat>` | Edit the node's `categories` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. `requisition category list` remains for inspection. |
| `provision.pl asset add <fs> <foreign-id> <name> <value>` | Edit the node's `spec.nodes[].assets` in `<fs>.yaml`, then `onmsctl apply -f <fs>.yaml`. Post-import, takes-effect-immediately escape hatch: `onmsctl requisition asset set <db-id> <field> <value>` (sibling reads: `asset list / get`). **Misfit:** keyed by integer database node ID, not foreign-id — resolve via `curl /opennms/rest/nodes?foreignId=<fid>` first. |

### Migrating off `provision.pl` shell automation

Recommended once-per-site recipe:

1. **Convert** existing XML to YAML:
   ```sh
   onmsctl requisition convert \
     --from /opt/opennms/etc/imports/ \
     --foreign-sources-dir /opt/opennms/etc/foreign-sources/ \
     --out repo/yaml/
   ```
   Review the stderr findings; resolve `PR001` / `PR002` by editing the source XML
   (rare) or accepting the documented data-loss (most common — see each code's
   `--explain` text).
2. **Commit** the YAML directory to git as the new source of truth.
3. **Rewrite** the existing `provision.pl` shell scripts as `onmsctl apply -f
   <fs>.yaml` invocations. The legacy "step-by-step mutation" pattern collapses to
   one apply per requisition.
4. **Schedule** the apply via CI / cron. `--dry-run --diff` is the review gate; the
   real apply runs only after review.

For ongoing sync — operators who edit requisitions in Horizon's UI and want to
pull the changes back into git — use `requisition export`:

```sh
onmsctl requisition export --out repo/yaml/        # every requisition, per-file
onmsctl requisition export acme-prod               # one requisition to stdout
onmsctl requisition export --include-defaults --out repo/yaml/   # inline default-FS
```

Without `--include-defaults`, the exported YAML omits `spec.foreignSource` when the
server has no custom FS (the portable style). With it, the default-FS is inlined
alongside a snapshot comment; that inlined block is a point-in-time copy that does
NOT stay in sync with Horizon's default after export.

## Legacy `users.xml` → `onmsctl`

| Pre-onmsctl | `onmsctl` |
|---|---|
| Hand-edit `$OPENNMS_HOME/etc/users.xml`, reload | `onmsctl apply -f users/` (one `kind: User` document per user; apply reconciles scalar fields + role set against the live server) |
| Add a user via the web UI | Add a `kind: User` document and `onmsctl apply -f` |
| Change a user's roles in the UI | Edit the document's `roles` set and `onmsctl apply -f` (roles reconcile as a set) |
| Rotate a password in the UI | `onmsctl iam user set-password <name> --password-stdin` |
| Remove a `<user>` element + reload | `onmsctl iam user delete <name> --yes` (idempotent; 404 → no-op) |

## Breaking changes

### `onmsctl requisition delete <fs>` requires `--yes` (since v0.1.1)

The verb purges both pending and deployed snapshots in a single call — a wider
blast radius than any other Write verb — so it refuses to run without explicit
acknowledgement:

- **Interactive (TTY) shells:** shows the requisition name + node count + last-import
  timestamp (ISO-8601 UTC) and prompts for `yes`/`y` (case-insensitive). Any other
  input — `no`, empty line, Ctrl-D/EOF — aborts with a **non-zero** exit so scripts
  can distinguish cancellation from success.
- **Non-interactive contexts (CI, scripted pipelines, redirected stderr):** refuses
  with an error pointing at `--yes`. There is no "auto-confirm because
  non-interactive" path. Both stdin AND stderr must be TTYs for the prompt to fire.

CI fix: add `--yes` (or `-y`) to existing invocations. If the requisition already
doesn't exist (both snapshots 404), the verb is a no-op and skips the prompt.
