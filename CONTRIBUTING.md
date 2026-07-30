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
CI runs `make licenses-check` on every pull request and fails if the committed report doesn't match the dependency tree.

## Local checks

Run `make verify` before opening a pull request. CI runs the same target.

If you touched anything under `.github/workflows/`, also run
`make lint-actions` — actionlint plus zizmor, the same gate CI applies.
It fetches both tools as pinned release binaries into `.bin/`, so it
needs no toolchain beyond `curl` and `tar`. It covers workflow syntax,
embedded shell, SHA pinning, least-privilege permissions, template
injection, and credential persistence. The runner-pin and SPDX-header
rules below are *not* machine-checked — they are on you.

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

Use [Conventional Commits](https://www.conventionalcommits.org/):
`<type>[scope]: <description>`, where type is one of `feat`, `fix`,
`docs`, `style`, `refactor`, `perf`, `test`, `chore`, `ci`, `build`, or
`revert`. Breaking changes append `!` or add a `BREAKING CHANGE:` footer.

## Sign-off (DCO)

Every commit **MUST** be signed off under the
[Developer Certificate of Origin](https://developercertificate.org/) 1.1:

```
git commit -s -m "fix(iam): ..."
```

That appends a trailer from your git identity:

```
Signed-off-by: Jane Developer <jane@example.org>
```

Signing off certifies that you wrote the contribution, or otherwise have
the right to submit it under Apache-2.0. It must carry a real name and a
reachable email — a pseudonym or a `noreply` address does not certify
anything.

Unsigned commits are not merged. If you forgot, `git commit --amend -s`
fixes the last one and `git rebase --signoff <base>` fixes a branch.

## AI-assisted contributions

AI assistance is welcome, under two rules.

**Disclose it.** Any commit produced with AI assistance **MUST** carry an
`Assisted-by:` trailer naming the agent and model:

```
Assisted-by: ClaudeCode:claude-opus-4-8
Signed-off-by: Jane Developer <jane@example.org>
```

**A human stays responsible.** Only the human submitter may add
`Signed-off-by:` — AI agents **MUST NOT** add one, because only a human
can certify the DCO. Signing off on AI-assisted work means you have
reviewed it, you understand it, and you are asserting it is
license-clean. In particular, the clean-room rule above binds AI-assisted
code exactly as it binds hand-written code: an agent must not be pointed
at OpenNMS server source.

Repository-wide guidance for coding agents lives in [`AGENTS.md`](AGENTS.md).
