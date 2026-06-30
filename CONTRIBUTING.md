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
- MPL-2.0  *(file-scope copyleft; acceptable)*
- Unicode-3.0 / Unicode-DFS-2016
- Unlicense
- Zlib

**Project-scope copyleft licenses (GPL, AGPL, LGPL) MUST NOT be introduced.**
File-scope copyleft (MPL-2.0) is acceptable: it imposes obligations on
modifications to MPL-licensed source files but does not restrict the
license of the wider work.

After adding a dependency, regenerate the third-party license report:

```
make licenses
```

and commit the updated `THIRD-PARTY-LICENSES.md`.

## Local checks

Run `make verify` before opening a pull request. CI runs the same target.

## CI runner images

GitHub Actions jobs pin their runner to an explicit version label (e.g. `ubuntu-24.04`,
`macos-26`), never a floating `-latest` alias, with a `# was <floating>` comment recording the
prior label. This keeps the build and release environment deterministic — a runner-image
migration cannot silently change a tag-triggered release.

Unlike `uses:` action SHAs, Dependabot does not manage `runs-on` labels, so these pins are
bumped **manually**. Revisit them when GitHub announces a pinned image's deprecation/removal
(its brownout period), and when adopting a newer OS image deliberately.

## Source file headers

Every new Rust source file starts with this SPDX-compliant header:

```rust
/*
 * Copyright <YEAR> Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */
```

Rules:

- `<YEAR>` is the file's creation year. Do not bump it on subsequent edits.
- The SPDX identifier line is load-bearing for license tooling — keep it
  exactly as `SPDX-License-Identifier: Apache-2.0`.
- The header sits at the very top of the file.
- Do not add this header to non-source files (Markdown, JSON, TOML, YAML).
- Do not add it to generated files (`// Code generated ... DO NOT EDIT.`).

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/). Every
commit assisted by an AI tool **MUST** include an `Assisted-by:` trailer
per the project's `CLAUDE.md`.

## Sign-off

Only the human submitter may add a `Signed-off-by:` trailer. AI agents
**MUST NOT** add one — only humans can certify the Developer Certificate
of Origin.
