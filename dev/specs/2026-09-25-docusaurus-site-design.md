# Docusaurus documentation site

Status: approved design, 2026-09-25.

## Goal

Move the operator documentation out of one long `README.md` and four flat files in `docs/` into a structured Docusaurus site.
The site is published with GitHub Pages at `https://onmsctl.no42.org`.

Success means a visitor finds install, context setup, one page per kind, and CLI reference through a sidebar.
They no longer scroll a 676-line README.

## Decisions

| Topic | Decision |
|---|---|
| Audience | Operators using `onmsctl`. Contributor material stays in the repo. |
| README | Slim landing page (about 60 lines). All depth moves to the site. |
| Versioning | Unversioned. One site built from `main`. Pages state minimum onmsctl or Horizon versions where it matters. |
| Domain | `onmsctl.no42.org` via CNAME to `no42-org.github.io`. |
| Layout | Docusaurus app in `website/`. Content stays in `docs/` and is read by the docs plugin with `path: '../docs'`. |
| Content | Reorganized, not rewritten. Existing prose is moved and split, with light edits for flow and links. |

## Information architecture

```
docs/
├── intro.mdx                    README pitch, latest-version badge, what it does, kubectl model
├── getting-started/
│   ├── install.md               README Install, Build from source, Container image + Quick Start §1-2
│   ├── configure-context.md     README "Configure a context" + Quick Start §3
│   └── quickstart.md            Quick Start §4-5 (core concepts, five-minute tour)
├── concepts/
│   ├── declarative-apply.md     README "Declarative apply"
│   └── read-only-contexts.md    README verb aliases and read-only gate
├── kinds/
│   ├── event-source.md          README EventSource + Quick Start §6 + eventsource-reference.md
│   ├── requisition.md           README Requisition + Quick Start §7
│   ├── user.md                  README User + Quick Start §8
│   ├── snmp-config.md           README SnmpConfig + Quick Start §9
│   ├── maintenance.md           README Maintenance + Quick Start §9c
│   ├── datacollection-source.md README DataCollectionSource + Quick Start §9b
│   └── business-service.md      README BusinessService + Quick Start §9d
├── guides/
│   ├── migration.md             docs/migration.md
│   └── troubleshooting.md       Quick Start §13
└── reference/
    ├── global-flags.md          Quick Start §10 (global flags, environment variables)
    ├── output-formats.md        README output formats + Quick Start §11
    ├── exit-codes.md            README exit codes + Quick Start §11
    ├── shell-completions.md     README shell completions + Quick Start §12
    ├── tls.md                   README TLS
    ├── editor-integration.md    yaml-language-server and schema URLs
    └── compatibility.md         README "Server compatibility" and known Horizon quirks
```

Each kind page covers the YAML shape, `apply`, the read and delete verbs, and `convert` where it exists.
Each kind page embeds its file from `examples/` so the examples stay the single source.

Other moves:

- `docs/manual-test-runbook.md` moves to `dev/manual-test-runbook.md`. `CONTRIBUTING.md` links to it.
- `CONTRIBUTING.md`, `RELEASING.md`, `SECURITY.md` and `SUPPORT.md` stay at the repo root. The site footer links to them on GitHub.
- JSON Schema URLs keep pointing at `raw.githubusercontent.com/no42-org/onmsctl/main/schemas/`. Existing editor setups keep working.
- Old anchors such as `docs/quickstart.md#6-...` break. That is acceptable: the project has no external users yet. Every in-repo link is updated.

### Slim README

The README keeps:

- the badges,
- the one-paragraph pitch and the pre-stability notice,
- the install one-liner,
- one short `apply -f` example,
- links into the site (Getting started, Kinds, Reference),
- License and Contributing sections.

## Toolchain

- Docusaurus 3, classic preset, TypeScript config.
- Docs plugin: `path: '../docs'`, `routeBasePath: '/'`. The blog is disabled.
- Local search with `@easyops-cn/docusaurus-search-local`. No external search service.
- Node version pinned in `website/.nvmrc`. `website/package-lock.json` is committed. Installs use `npm ci`.
- `onBrokenLinks: 'throw'`, `onBrokenMarkdownLinks: 'throw'`, `onBrokenAnchors: 'throw'`.
- `url: 'https://onmsctl.no42.org'`, `baseUrl: '/'`. `website/static/CNAME` contains `onmsctl.no42.org`.
- New source files in `website/` (`docusaurus.config.ts`, `sidebars.ts`, `src/css/custom.css`, `src/components/VersionBadge.tsx`) carry the SPDX Apache-2.0 header. Markdown does not.
- `website/node_modules/` and `website/build/` and `website/.docusaurus/` are gitignored.

## Theme

- Stock classic theme. Colour mode follows the system setting.
- Navbar: site title, Docs, GitHub link.
- No custom React pages. `intro.mdx` is the landing page.
- Footer link columns:
  - Docs: Getting started, Kinds, Reference.
  - Project: GitHub, Releases, Contributing, Security policy.
- Footer line, as `footer.copyright` HTML:

  ```html
  Made with ❤️ in Heilbronn, Germany, Europe 🇪🇺 by <a href="https://blog.no42.org/page/about/">Ronny Trommer</a> · <a href="https://ko-fi.com/indigo423">buy me a coffee</a>
  ```

## Latest-version badge

The landing page shows the latest release as a badge, modelled on `riptide.space`.

- `docusaurus.config.ts` resolves the version at build start from git tags: `git tag --list 'v*' --sort=-version:refname`, keeping only stable tags that match `^v[0-9]+\.[0-9]+\.[0-9]+$`. The result goes into `customFields.latestVersion`.
- If no stable tag is found (for example in a shallow clone), the build fails with a message saying tags are required.
- `website/src/components/VersionBadge.tsx` reads `customFields.latestVersion` and renders a pill link, `vX.Y.Z ↗`, to `https://github.com/no42-org/onmsctl/releases/tag/vX.Y.Z`. It carries an `aria-label` naming the release notes and a `focus-visible` outline.
- Badge styles live in `src/css/custom.css` and use Infima colour variables so light and dark mode both work.
- `docs/intro.mdx` renders `<VersionBadge />` above the pitch. The badge appears on the landing page only.
- Assumption: a pushed `v*` tag means a published release. A tag whose release stays a draft would be advertised. The release workflow publishes on tag push, so this holds in normal operation.

## Build and CI

Makefile targets (CI calls only these):

- `make docs`: `npm ci` then `npm run build` in `website/`.
- `make docs-serve`: local dev server with live reload.

Workflows:

- `gates.yml` gains a `docs` job on `ubuntu-24.04` running `make docs`. PRs, `main` pushes and releases enforce a buildable site through the existing gate. It is a separate job so a docs failure is not reported as a Rust failure.
- The `gates.yml` docs job checks out with `fetch-depth: 0` and `fetch-tags: true` so the version badge resolves.
- New `docs.yml` deploys the site:
  - Triggers: push to `main` touching `docs/**`, `website/**` or `examples/**`, push of a `v*` tag (so the badge updates on every release), plus `workflow_dispatch`.
  - Checkout uses `fetch-depth: 0` and `fetch-tags: true`.
  - Jobs: `build` runs `make docs` and `actions/upload-pages-artifact`. `deploy` runs `actions/deploy-pages`.
  - Permissions: `contents: read` at the top. `pages: write` and `id-token: write` only on `deploy`.
  - `environment: github-pages`, `concurrency: { group: pages, cancel-in-progress: false }`.
  - `checkout` uses `persist-credentials: false`.
- All actions pinned to a commit SHA with the full semver in the trailing comment.
- `make lint-actions` (actionlint and zizmor) must pass on the new workflow.

Dependency updates:

- `.github/dependabot.yml` gains an `npm` entry for `/website`: weekly, grouped minor and patch updates, commit prefix `build`, scope included, label `dependencies`.
- Renovate stays limited to Makefile tool pins.

## Manual steps for the maintainer

1. Settings, Pages: set source to "GitHub Actions".
2. DNS: `onmsctl.no42.org CNAME no42-org.github.io`.
3. Settings, Pages: set the custom domain, then enable "Enforce HTTPS" once the certificate is issued.
4. Set the repository homepage URL to `https://onmsctl.no42.org`.

## Verification

- `make docs` passes with all broken-link and broken-anchor checks on.
- The built landing page shows the badge with the highest stable tag, linking to that release.
- Coverage check: every `##` section of the old `README.md`, `quickstart.md`, `migration.md` and `eventsource-reference.md` maps to a page. The implementation plan carries this mapping as a checklist.
- `make verify` and `make lint-actions` stay green.
- After merge and the manual steps, `https://onmsctl.no42.org` serves the site over HTTPS.

## Out of scope

- Versioned docs.
- Generated CLI reference from clap. A later change can add it.
- Rendering JSON Schemas as pages.
- A blog or release-notes section.
