# Contributing to onmsctl

Thanks for considering a contribution. A few load-bearing rules below.

## Clean-room implementation

`onmsctl` is licensed under Apache-2.0 and interoperates with the OpenNMS
Horizon REST API. To preserve license clarity, contributors **MUST** follow
clean-room separation:

- Implementation work proceeds from the published OpenAPI document, the
  project's own design artifacts, and black-box observation of a running
  Horizon instance.
- Contributors **MUST NOT** consult, paraphrase, or transcribe Horizon's
  server source code while writing `onmsctl` Rust code.
- Where additional Horizon behaviour needs to be characterized, it is
  characterised via black-box interaction with a running instance and added
  to the project's design notes — never by reading server source.

## Third-party crates

Pull requests adding new crate dependencies are reviewed for license
compatibility. The allowed license set is enforced by `cargo deny check`
(see `deny.toml`):

- Apache-2.0
- MIT
- BSD-2-Clause / BSD-3-Clause
- ISC
- MPL-2.0
- Unicode-3.0 / Unicode-DFS-2016
- Unlicense
- Zlib

Copyleft licenses (GPL, AGPL, LGPL) **MUST NOT** be introduced.

After adding a dependency, regenerate the third-party license report:

```
make licenses
```

and commit the updated `THIRD-PARTY-LICENSES.md`.

## Local checks

Run `make verify` before opening a pull request. CI runs the same target.

## Source file headers

Every new Rust source file starts with the SPDX-compliant header documented
in the project's `CLAUDE.md`. The header is short and stable across edits.

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/). Every
commit assisted by an AI tool **MUST** include an `Assisted-by:` trailer
per the project's `CLAUDE.md`.

## Sign-off

Only the human submitter may add a `Signed-off-by:` trailer. AI agents
**MUST NOT** add one — only humans can certify the Developer Certificate
of Origin.
