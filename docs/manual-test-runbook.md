# onmsctl — Manual Test Runbook (local verification)

A step-by-step runbook for verifying a local `onmsctl` build by hand. It
complements the automated suites:

- `make test` — unit/wiremock tests, no server.
- `make integration` — the `#[ignore]`d live-Horizon tests (needs `ONMSCTL_TEST_*`).

This runbook is for **interactive, eyes-on** verification of the CLI surface —
especially the declarative `onmsctl apply -f` path and the kept imperative verbs.

---

## 0. Conventions

- `BIN` is the binary under test. Set it once:
  ```sh
  export BIN=./target/debug/onmsctl
  ```
- Every server-side resource this runbook creates is prefixed **`onmsctl-rb-`**
  so the cleanup step (§8) can find and remove it without touching real data.
- After a command, check the shell exit code with `echo "exit=$?"`. Stable exit
  codes are listed in §7.
- **Offline** steps (§2–§3) need no Horizon. **Live** steps (§4–§6) need a
  reachable Horizon and write to it — run them only against a lab/dev instance.

---

## 1. Prerequisites

### Local tooling (all steps)

- **Rust toolchain** pinned by `rust-toolchain.toml` (currently **1.95**) —
  `rustup` will auto-select it in the repo.
- **`make`** — the runbook builds via `make build`.
- A **POSIX shell** (`bash` or `zsh`). The examples use heredocs, `$(…)`,
  `mktemp -d`, and `sed -i.bak` — all macOS- and Linux-compatible as written.
- **`python3`** — used by the cleanup sweep (§8) and a couple of JSON filters.
- **`git`** — to obtain and build the source.
- **macOS or Linux.** On macOS, if you use a `keyring:` credential the system
  Keychain must be unlocked/reachable (a locked or dark-wake keychain can block
  server verbs — env-var creds avoid this).

### Build

```sh
make build            # → target/debug/onmsctl
export BIN=./target/debug/onmsctl
$BIN version          # sanity-check the build before continuing (see §2)
```
(For a release-profile binary instead: `cargo build --release` → `target/release/onmsctl`.)

### For the live steps (§4–§6)

- A reachable **OpenNMS Horizon 35+** instance — you'll point `onmsctl` at its
  `…/opennms` REST root.
- An account that can **create and delete users, eventconf sources, and
  requisitions**: **`ROLE_ADMIN`**, or at minimum `ROLE_PROVISION` + `ROLE_REST`
  + user-administration rights.
- Network reachability to the Horizon REST API (no auth-stripping proxy in the path).
- Credentials supplied via env (`ONMS_URL` / `ONMS_USER` / `ONMS_PASSWORD`) or a
  context in `config.yaml` (§4).
- **Use a lab / dev instance — never production.** §5–§6 create and then delete
  `onmsctl-rb-*` resources on the server; the cleanup step (§8) removes them, but
  a crash mid-run can leave prefixed leftovers.

### Known server-side quirks to expect (so results aren't misread as failures)

- **NMS-19810** — `event-source list` can return empty even when sources exist. Use
  `event-source names-and-ids` to verify eventconf state (the runbook already does).
- **NMS-19813** — the eventconf upload needs the `name="upload"` multipart
  workaround, which `onmsctl` applies automatically; a raw upload by other tools
  may 400. (This is the bug gating the v0.1.0 release.)
- **New-user auth lag** — a freshly REST-created user may 401 on basic auth until
  Horizon's realm refreshes. Verify new users with `iam user get` (or the
  server-side hash), not by authenticating as them.

---

## 2. Build & smoke (offline)

```sh
make build                      # → target/debug/onmsctl
export BIN=./target/debug/onmsctl

$BIN version                    # expect: onmsctl + eventconf/provisioning/iam
$BIN --help                     # expect top-level: apply, source, event,
                                #   requisition, iam, version, config, completion
$BIN apply --help               # expect flags: -f, --dry-run, --diff,
                                #   --continue-on-error (alias --keep-going), -R
```

**Pass:** `version` lists three capabilities; `apply --help` shows exactly those
five flags; no panic.

---

## 3. Offline verification (no Horizon required)

These exercise the pure-local verbs and the `apply` gate paths that fail
**before any HTTP**. Use a throwaway config so context resolution succeeds but
no server is ever contacted:

```sh
export RB=$(mktemp -d)
cat > "$RB/config.yaml" <<'EOF'
current-context: rb
contexts:
  - name: rb
    server: {url: http://127.0.0.1:9/opennms}   # unreachable on purpose
    auth: {basic: {username: admin, password: admin}}
EOF
CFG="--config $RB/config.yaml"
```

### 3.1 Local XML→YAML converters (no context, no network)

```sh
# Requisition convert — emits YAML + PR### findings on stderr.
$BIN requisition convert --explain PR001          # prints rule rationale, exit 0
# Source convert — emits EventSource YAML + EC### findings.
$BIN event-source convert --explain EC001               # prints rule rationale, exit 0
```
**Pass:** both print rationale text and exit 0. (Convert runs with **no** config
and **no** keyring — verify by adding `--config /no/such/file`; it still works.)

### 3.2 Config

```sh
$BIN $CFG config view                # prints config, secrets redacted, exit 0
$BIN $CFG config use-context rb       # "switched to context 'rb'", exit 0
$BIN $CFG config use-context nope      # error: not found, exit 2
```

### 3.3 Shell completion

```sh
$BIN completion bash | head -3        # emits a completion script, exit 0
```

### 3.4 `apply` gate paths (fail before any HTTP)

```sh
# Unknown kind → plan gate aborts before contacting the server.
printf 'apiVersion: v1\nkind: Bogus\nmetadata: {name: x}\n' > "$RB/bad.yaml"
$BIN $CFG apply -f "$RB/bad.yaml" --dry-run; echo "exit=$?"   # names the kind, exit 1

# Empty / comment-only input → usage error.
printf '# nothing here\n' > "$RB/empty.yaml"
$BIN $CFG apply -f "$RB/empty.yaml" --dry-run; echo "exit=$?"  # "no YAML documents", exit 2

# Missing -f → clap usage error.
$BIN $CFG apply; echo "exit=$?"                                # required-arg error, exit 2

# -R with a non-directory -f → stderr note, then proceeds.
$BIN $CFG apply -R -f "$RB/bad.yaml" --dry-run 2>&1 | grep -i recursive
```
**Pass:** unknown-kind exits **1** and names `"Bogus"`; empty/missing exit **2**;
the `-R` note fires. None of these reach `127.0.0.1:9` (no connection error).

### 3.5 Read-only context gate (exit 12, before HTTP)

```sh
$BIN $CFG --read-only apply -f "$RB/bad.yaml"; echo "exit=$?"  # refused locally, exit 12
$BIN $CFG --read-only apply -f "$RB/bad.yaml" --dry-run; echo "exit=$?"  # dry-run = Read → NOT refused (then hits the unknown-kind gate)
```
**Pass:** a real (non-dry-run) apply under `--read-only` exits **12** with no HTTP;
`--dry-run` is classified Read and is allowed through to the gate.

---

## 4. Live Horizon setup

Point a context at your lab (replace the URL/creds), or use env vars:

```sh
export ONMS_URL=https://horizon.dev.lab/opennms
export ONMS_USER=admin
export ONMS_PASSWORD=…            # or use a keyring/password-file in config.yaml
export BIN=./target/debug/onmsctl

$BIN whoami 2>/dev/null || $BIN iam whoami   # prints the calling user → creds work
```
**Pass:** `iam whoami` prints your username. If it 401s, fix creds before
proceeding. (Heads-up: a freshly REST-created user may 401 until Horizon's auth
realm refreshes — verify new users via the server hash, not by logging in as them.)

> All §5–§6 resources are named `onmsctl-rb-*`; §8 cleans them up.

---

## 5. Live — declarative apply (`onmsctl apply -f`, the canonical path)

Stage a directory mixing all three kinds. The `User` doc needs a password source;
reuse `$ONMS_PASSWORD`:

```sh
export DESIRED=$(mktemp -d)

cat > "$DESIRED/10-user.yaml" <<'EOF'
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata: {name: onmsctl-rb-user}
spec:
  fullName: Runbook User
  roles: [ROLE_USER]
  passwordRef: {fromEnv: ONMS_PASSWORD}
EOF

cat > "$DESIRED/20-source.yaml" <<'EOF'
apiVersion: eventconf.opennms.org/v1
kind: EventSource
metadata: {name: onmsctl-rb.src}
spec:
  enabled: true
  events:
    - uei: uei.opennms.org/onmsctl-rb/first
      label: First
      severity: Warning
EOF

cat > "$DESIRED/30-req.yaml" <<'EOF'
apiVersion: provisioning.opennms.org/v1
kind: Requisition
metadata: {name: onmsctl-rb-req}
spec:
  nodes:
    - foreignId: web01
      label: web01.onmsctl-rb
EOF
```

### 5.1 Dry-run first (no mutation)

```sh
$BIN apply -f "$DESIRED/" --dry-run -o table; echo "exit=$?"
```
**Pass:** three rows, statuses `Skipped`/`Unchanged` (predicted actions
`create`), exit **0**. Confirm nothing was created: `iam user get onmsctl-rb-user`
should 404 / "does not exist".

### 5.2 Real apply — creates all three, in precedence order

```sh
$BIN apply -f "$DESIRED/" -o table; echo "exit=$?"
```
**Pass:** processed **User → EventSource → Requisition** (rank 100/200/300);
each row `Created`; exit **0**. Verify:
```sh
$BIN iam user get onmsctl-rb-user
$BIN event-source names-and-ids | grep onmsctl-rb     # source list may be empty (NMS-19810); use names-and-ids
$BIN requisition get onmsctl-rb-req 2>/dev/null || $BIN requisition status onmsctl-rb-req
```

### 5.3 Idempotent re-apply

```sh
$BIN apply -f "$DESIRED/" -o table
```
**Pass:** the EventSource/Requisition rows report `Unchanged` on re-run
(handlers are idempotent). (User may re-report `Updated` if the server normalizes
fields — note it, not a failure.)

### 5.4 Update detection

```sh
# Add a second event to the source, re-apply.
sed -i.bak 's/severity: Warning/severity: Warning\n    - uei: uei.opennms.org\/onmsctl-rb\/second\n      label: Second\n      severity: Major/' "$DESIRED/20-source.yaml"
$BIN apply -f "$DESIRED/20-source.yaml" -o table
```
**Pass:** the source row reports `Updated`.

### 5.5 Diff

```sh
$BIN apply -f "$DESIRED/20-source.yaml" --dry-run --diff
```
**Pass:** a UEI-bucketed diff prints to **stderr**; stdout stays the outcome
rows (so `-o json` consumers are unaffected).

### 5.6 stop-on-error vs continue-on-error

Introduce a doc that fails at execute (e.g. a User whose `passwordRef` points at
an unset env var), then compare:

```sh
cat > "$DESIRED/05-bad-user.yaml" <<'EOF'
apiVersion: onmsctl.no42.org/v1alpha1
kind: User
metadata: {name: onmsctl-rb-baduser}
spec: {fullName: Bad, roles: [ROLE_USER], passwordRef: {fromEnv: DEFINITELY_UNSET}}
EOF

$BIN apply -f "$DESIRED/"; echo "exit=$?"                       # default: stop-on-error
$BIN apply -f "$DESIRED/" --continue-on-error; echo "exit=$?"   # attempts every bucket
```
**Pass:** both exit **1** (a document failed). Under default, later buckets show
`Skipped` (not attempted); under `--continue-on-error`, every bucket is attempted
and only the bad one is `Failed`. (Remove `05-bad-user.yaml` before re-running clean.)

### 5.7 Recursive directory

```sh
mkdir -p "$DESIRED/sub" && cp "$DESIRED/30-req.yaml" "$DESIRED/sub/40-req2.yaml"
sed -i.bak 's/onmsctl-rb-req/onmsctl-rb-req2/' "$DESIRED/sub/40-req2.yaml"
$BIN apply -f "$DESIRED/" --dry-run            # non-recursive: 3 docs (ignores sub/)
$BIN apply -f "$DESIRED/" -R --dry-run         # recursive: 4 docs (includes sub/)
```
**Pass:** without `-R`, `sub/` is ignored; with `-R`, the nested doc is included.

---

## 6. Live — kept imperative verbs (reads, explicit deletes, round-trips)

Mutation moved to `apply -f`; these are the verbs that stay imperative.

### 6.1 eventconf

```sh
$BIN event-source names-and-ids                 # {id,name} listing (works despite NMS-19810)
$BIN event-source get <id>                      # one source
$BIN event-source download <id> -O /tmp/rb.xml  # raw eventconf XML
$BIN event-source upload /tmp/rb.xml            # raw upload round-trip
$BIN event list --source <id>             # events for a source (read-only)
$BIN event-source delete <id>                   # explicit delete (kept)
```

### 6.2 provisioning

```sh
$BIN requisition list
$BIN requisition status onmsctl-rb-req
$BIN requisition export onmsctl-rb-req               # server-state → declarative YAML
$BIN requisition node list onmsctl-rb-req            # read-only sub-resource: positional <fs>
$BIN requisition node get  onmsctl-rb-req web01      # <fs> <foreign-id>
# interface/service/category reads are similarly positional, scoped one level deeper:
#   requisition interface list <fs> <foreign-id>
#   requisition service  list <fs> <foreign-id> <ip>
#   requisition category list <fs> <foreign-id>
$BIN requisition import onmsctl-rb-req --wait --timeout 2m   # trigger + block on import
$BIN requisition delete onmsctl-rb-req --yes         # --yes REQUIRED in non-TTY
```
**Pass:** `node`/`interface`/`service`/`category` expose only `list`/`get`
(no `add`/`set`/`remove`); `requisition delete` without `--yes` in a
non-interactive shell refuses with exit **2**.

### 6.3 iam

```sh
$BIN iam whoami
$BIN iam user list
$BIN iam user get onmsctl-rb-user
$BIN iam user export --name onmsctl-rb-user          # round-trips through apply
printf %s "$ONMS_PASSWORD" | $BIN iam user set-password onmsctl-rb-user --password-stdin
$BIN iam user delete onmsctl-rb-user --yes
```
**Pass:** `iam user` exposes only `list`/`get`/`delete`/`set-password`/`export`
(no `create`/`update`/`role`). `iam apply` no longer exists.

### 6.4 Verify removed verbs are gone

```sh
$BIN requisition apply -f x.yaml   2>&1 | head -1   # expect: unrecognized subcommand
$BIN event-source apply -f x.yaml        2>&1 | head -1   # expect: unrecognized subcommand
$BIN iam apply -f x.yaml           2>&1 | head -1   # expect: unrecognized subcommand
$BIN iam user create alice         2>&1 | head -1   # expect: unrecognized subcommand
```
**Pass:** all four report an unrecognized-subcommand clap error (exit 2).

---

## 7. Exit-code spot checks

| Exit | Trigger to verify |
|---|---|
| 0 | `apply -f` where all docs apply or are unchanged |
| 1 | `apply -f` with any failing doc, the plan gate, or unknown kind (§3.4, §5.6) |
| 2 | usage error: missing `-f`, empty input, `delete` without `--yes` (non-TTY) |
| 4–8 | transport: point a context at a dead host / bad DNS / TLS-broken endpoint |
| 12 | Write verb under a `read-only` context / `--read-only` (§3.5) |
| 13/14/15 | IAM admin-lockout / self-lockout / `whoami`-unavailable during a `kind: User` apply |

```sh
# Transport example (exit 4/5): unreachable host.
$BIN --url http://10.255.255.1:9/opennms --user x event-source names-and-ids; echo "exit=$?"
```

---

## 8. Cleanup

```sh
# Remove any onmsctl-rb-* resources left on the server.
for n in $($BIN requisition list 2>/dev/null | grep onmsctl-rb); do
  $BIN requisition delete "$n" --yes
done
$BIN iam user list -o json 2>/dev/null | grep -o 'onmsctl-rb[^"]*' | sort -u | while read u; do
  $BIN iam user delete "$u" --yes
done
$BIN event-source names-and-ids -o json 2>/dev/null \
  | python3 -c 'import sys,json;[print(s["id"]) for s in json.load(sys.stdin) if s["name"].startswith("onmsctl-rb")]' \
  | while read id; do $BIN event-source delete "$id"; done

# Local temp dirs.
rm -rf "$RB" "$DESIRED"
```

**Done** when `requisition list`, `iam user list`, and `event-source names-and-ids`
show no `onmsctl-rb-*` entries.
