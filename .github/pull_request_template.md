<!--
Work starts from an issue, not a drive-by PR. If there isn't one yet,
please open it first so the change can be discussed before review.
-->

Closes #

## What changed

<!-- What this does, and why. Not a restatement of the diff. -->

## How it was verified

<!--
The command you ran and what it printed — `make verify` at minimum.
For behaviour changes, say what you exercised against a live Horizon
instance, or why that wasn't applicable.
-->

## Checklist

- [ ] `make verify` passes locally (fmt, clippy, build, test, cargo-deny).
- [ ] Commits follow [Conventional Commits](https://www.conventionalcommits.org/)
      and are signed off (`git commit -s`). AI-assisted commits carry an
      `Assisted-by:` trailer; only a human adds `Signed-off-by:`.
- [ ] New source files carry the SPDX header (see [CONTRIBUTING.md](../CONTRIBUTING.md)).
- [ ] Docs updated — README, `docs/`, and `RELEASING.md` if the release
      pipeline changed.
- [ ] New dependencies are license-compatible and `make licenses` was re-run.
- [ ] Breaking changes to CLI flags, config schema, or manifest schema are
      called out above and marked `!` / `BREAKING CHANGE:` in the commit.
