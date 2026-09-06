# AGENTS.md

`onmsctl` is a `kubectl`-style CLI for the OpenNMS Horizon REST API.
Rust workspace, Apache-2.0, pre-1.0.

## Commands

`make help` lists everything. The ones that matter:

| Command | Does |
|---|---|
| `make verify` | The full gate: fmt + clippy + build + test + deny. **Run before every PR** — CI runs this exact target. |
| `make test` | Workspace unit + doc tests. |
| `make lint-actions` | Lint `.github/workflows` (actionlint + zizmor). Also a CI gate. |
| `make schema` | Regenerate the committed `schemas/*.schema.json`. |
| `make licenses` | Regenerate `THIRD-PARTY-LICENSES.md` after a dependency change. CI (`licenses-drift`) fails on a stale report. |
| `make integration` | Live-Horizon tests. Needs `ONMSCTL_TEST_URL` / `_USER` / `_PASSWORD`. |
| `make fuzz` | Run one cargo-fuzz harness from `fuzz/` on nightly (`FUZZ_TARGET=`, `FUZZ_SECS=`). `make fuzz-check` runs fmt, clippy and cargo-deny on them on stable and is a CI job. |

Single test: `cargo test -p onmsctl-core peek_kind` (add `-- --nocapture` for output).

## Architecture

`crates/onmsctl` is the binary; everything else is a library crate.
`crates/onmsctl-core` holds what is shared: the `kind` router
(`src/kind/`), config/context resolution, the HTTP client, and the
`CmdKind` read/write classification (`src/cmd.rs`). Each remaining crate
is one capability — `-eventconf`, `-provisioning`, `-iam`, `-snmp`,
`-maintenance`, `-datacollection`, `-businessservice` — owning its
models, REST calls, and JSON Schema. `crates/onmsctl-it` is live-server
integration tests only.

`apply -f` is the single mutation entrypoint. It parses every YAML
document, peeks `kind` (`kind/envelope.rs`), orders documents by
dependency rank (`kind/precedence.rs`), and dispatches through a
registry of handlers (`kind/router.rs`, `kind/registry.rs`). There is
no per-capability `apply` verb. Reads, deletes and `convert` stay
imperative under their capability subcommand.

## Gotchas

- **Clean-room rule, non-negotiable.** Never consult, paraphrase, or
  transcribe OpenNMS server source while writing `onmsctl` code. Work
  from the published OpenAPI document, project design notes, and
  black-box observation of a running instance. See `CONTRIBUTING.md`.
- **Every new source file needs the SPDX header** (`CONTRIBUTING.md`).
  Not on Markdown/JSON/TOML/YAML, not on generated files.
- **Every new command must classify itself** as `CmdKind::Read` or
  `Write`. That classification is what the `--read-only` gate enforces
  locally, before any HTTP is issued. A pure local transform is `Read`.
- **Schemas are generated, not hand-edited.** Per-crate `schema_drift`
  tests fail CI when a committed schema falls behind its Rust types —
  change the types, then run `make schema`.
- **Integration tests are `#[ignore]`d and run serially**; they skip
  themselves when the env vars are unset. `make test` never runs them.
- **`runs-on` labels are pinned deliberately** (`ubuntu-24.04`, not
  `-latest`) and Dependabot does not manage them. Don't "modernize"
  them; see `CONTRIBUTING.md`.
- **Capabilities on unreleased endpoints need a version gate** with an
  error naming the required Horizon version — see `TRAPD_UNSUPPORTED`
  in `crates/onmsctl-snmp/src/api.rs` for the shape.
- **Commits**: Conventional Commits, signed off (`git commit -s`), with
  an `Assisted-by:` trailer for AI-assisted work. Only a human adds
  `Signed-off-by:`.
