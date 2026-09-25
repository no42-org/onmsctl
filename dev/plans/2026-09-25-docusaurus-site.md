# Docusaurus Documentation Site Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 676-line README and the flat `docs/*.md` files with a structured Docusaurus site at `https://onmsctl.no42.org`, deployed by GitHub Pages from `main`.

**Architecture:** A Docusaurus 3 app lives in `website/` and reads Markdown from the repo-root `docs/` via `path: '../docs'`, served at the site root. The latest release version is resolved from git tags when the build starts and rendered as a badge on the landing page. CI builds the site in the shared `gates.yml` gate; a new `docs.yml` deploys it with the official Pages actions.

**Tech Stack:** Docusaurus 3.10.2 (classic preset, TypeScript config), React 19.3.0, Node 24, `@easyops-cn/docusaurus-search-local` 0.55.3, `raw-loader` 4.0.2, `node:test` for the one unit-tested module, GNU Make, GitHub Actions.

**Spec:** `dev/specs/2026-09-25-docusaurus-site-design.md`

## Global Constraints

- Branch: `docs/docusaurus-site`. Every commit: Conventional Commits, `git commit -s`, trailer `Assisted-by: ClaudeCode:claude-opus-5-5`.
- Site URL `https://onmsctl.no42.org`, `baseUrl: '/'`, docs `routeBasePath: '/'`, blog disabled.
- `onBrokenLinks: 'throw'`, `onBrokenAnchors: 'throw'`, `markdown.hooks.onBrokenMarkdownLinks: 'throw'`.
- `markdown.format: 'detect'`: `.md` is parsed as CommonMark, `.mdx` as MDX. Use `.mdx` only for pages that import components or files.
- Node pinned by `website/.nvmrc` (`24`). `website/package-lock.json` committed. Installs use `npm ci`.
- CI invokes Makefile targets only (`make docs`), never `npm` directly.
- Actions pinned by full SHA with full semver comment. Pins to use:
  - `actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1`
  - `actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0`
  - `actions/configure-pages@45bfe0192ca1faeb007ade9deae92b16b8254a0d # v6.0.0`
  - `actions/upload-pages-artifact@fc324d3547104276b827a68afc52ff2a11cc49c9 # v5.0.0`
  - `actions/deploy-pages@368f82528645a54fb793d4d04e342629a3f51346 # v5.0.1`
- `runs-on: ubuntu-24.04` (pinned label, never `-latest`).
- New source files in `website/` (`.ts`, `.tsx`, `.css`) start with:
  ```
  /*
   * Copyright 2026 Ronny Trommer <ronny@no42.org>
   * SPDX-License-Identifier: Apache-2.0
   */
  ```
  Markdown, JSON and YAML files get no header.
- Repo Markdown style (applies to every page you write or move): one sentence per line, no hard wrapping; no em-dashes (rewrite as short sentences); no AI-slop tell words. Code blocks are copied verbatim.
- Badge renders on the landing page only.
- Footer copyright HTML, verbatim: `Made with ❤️ in Heilbronn, Germany, Europe 🇪🇺 by <a href="https://blog.no42.org/page/about/">Ronny Trommer</a> · <a href="https://ko-fi.com/indigo423">buy me a coffee</a>`
- JSON Schema URLs stay `https://raw.githubusercontent.com/no42-org/onmsctl/main/schemas/<name>.schema.json`.

## Review Focus

1. Pre-release tags (`v0.5.0-rc1`) and two-digit components (`v0.10.0` vs `v0.9.9`): the badge must show the highest stable version by numeric order. Pinned by unit tests in Task 2.
2. A checkout without tags (shallow clone, `fetch-depth: 1`): the build must fail with a message that says tags are required, not render `vundefined`. Pinned by a manual repro step in Task 2.
3. Migrated prose containing `<placeholder>` or `{...}` outside code spans inside `.mdx` pages: must not break the MDX parse. Caught by `make docs`; Task 4 lists the escape rule.
4. Old in-repo links (`docs/quickstart.md#...`, `README.md#...`) in SUPPORT, SECURITY, examples/README and the issue template: must point to a live site page. Pinned by the URL check script in Task 6.
5. A `v*` tag push with no docs change: the site must redeploy so the badge updates. Pinned by the `tags` trigger in Task 7 and checked in Task 8.

---

## File map

| Path | Responsibility |
|---|---|
| `website/package.json`, `package-lock.json`, `.nvmrc`, `tsconfig.json` | Node toolchain for the site |
| `website/docusaurus.config.ts` | Site config: URL, docs plugin, search, navbar, footer, version field |
| `website/sidebars.ts` | Explicit sidebar order |
| `website/src/latest-version.ts` | Pure tag picker plus git wrapper |
| `website/src/latest-version.test.ts` | `node:test` unit tests for the tag picker |
| `website/src/components/VersionBadge.tsx` | Landing-page release badge |
| `website/src/css/custom.css` | Badge styles |
| `website/static/CNAME` | Custom domain for Pages |
| `docs/**` | Site content (see spec information architecture) |
| `dev/legacy-docs/` | Temporary home of the old docs during migration; deleted in Task 5 |
| `dev/manual-test-runbook.md` | Contributor runbook, moved out of `docs/` |
| `Makefile` | `docs`, `docs-serve` targets |
| `.github/workflows/gates.yml` | `docs` job in the shared gate |
| `.github/workflows/docs.yml` | Pages deploy |
| `.github/dependabot.yml` | `npm` entry for `/website` |
| `scripts/check-site-urls.sh` | Checks every `onmsctl.no42.org` URL in tracked files resolves in the build |

## Source map (line numbers on `main` at `7fe71df`)

`README.md`:

| Lines | Section |
|---|---|
| 9-16 | pitch |
| 21-23 | pre-stability notice |
| 42-96 | Install |
| 97-123 | Build from source |
| 124-185 | Container image |
| 186-227 | Configure a context |
| 228-284 | Declarative apply |
| 285-314 | Event configuration |
| 315-352 | Provisioning |
| 353-388 | Users and roles |
| 389-422 | SNMP configuration |
| 423-477 | Maintenance windows |
| 478-506 | SNMP data collection |
| 507-573 | Business services |
| 576-592 | Editor integration |
| 593-596 | Output formats |
| 597-607 | Verb aliases & read-only contexts |
| 608-622 | Exit codes |
| 623-635 | Shell completions |
| 636-642 | TLS |
| 643-666 | Server compatibility |

`docs/quickstart.md` (moved to `dev/legacy-docs/quickstart.md` in Task 1):

| Lines | Section |
|---|---|
| 1-17 | intro |
| 39-48 | §1 Prerequisites |
| 49-102 | §2 Install |
| 103-173 | §3 Configure a context |
| 174-210 | §4 Core concepts |
| 211-235 | §5 Five-minute tour |
| 236-290 | §6 Event configuration |
| 291-364 | §7 Requisitions |
| 365-408 | §8 IAM users |
| 409-440 | §9 SNMP configuration |
| 441-466 | §9b SNMP data collection |
| 467-488 | §9c Maintenance windows |
| 489-513 | §9d Business services |
| 514-542 | §10 Global flags and environment variables |
| 543-571 | §11 Output formats and exit codes |
| 572-586 | §12 Shell completion |
| 587-617 | §13 Troubleshooting |

`docs/migration.md` and `docs/eventsource-reference.md` move whole.

## Migration rules (Tasks 3-5)

Apply to every page built from legacy prose:

1. Start each page with front matter: `title:` and a one-sentence `description:`. The `title` renders as the H1, so do not repeat it as `#`.
2. Where README and Quick Start cover the same thing, keep the more complete text once. Never keep two copies.
3. Drop Quick Start section numbers ("§7") and in-page "Contents" lists. The sidebar and the table of contents replace them.
4. Links between pages: relative file paths with extension, e.g. `[Requisition](../kinds/requisition.mdx#convert)`. The build checks them.
5. Links to `examples/`, `schemas/`, `CONTRIBUTING.md` or other repo files: absolute GitHub URLs, `https://github.com/no42-org/onmsctl/blob/main/<path>`.
6. One sentence per line. Remove em-dashes by splitting into short sentences. Copy code blocks verbatim.
7. In `.mdx` files, prose outside code spans must not contain bare `<` or `{`. Wrap the token in backticks (`` `<fs>` ``) or escape it (`\{`).
8. Keep the facts. Do not add claims that are not in the source.

---

### Task 1: Scaffold the site and build target

**Files:**
- Create: `website/package.json`, `website/.nvmrc`, `website/tsconfig.json`, `website/docusaurus.config.ts`, `website/sidebars.ts`, `website/src/css/custom.css`, `website/static/CNAME`, `docs/intro.md`
- Move: `docs/manual-test-runbook.md` → `dev/manual-test-runbook.md`; `docs/quickstart.md`, `docs/migration.md`, `docs/eventsource-reference.md` → `dev/legacy-docs/`
- Modify: `Makefile`, `.gitignore`

**Interfaces:**
- Produces: `make docs` (install, test, build into `website/build`), `make docs-serve`; sidebar id `docs`; doc id `intro` at slug `/`.

- [ ] **Step 1: Move the legacy files out of the content root**

```bash
mkdir -p dev/legacy-docs
git mv docs/manual-test-runbook.md dev/manual-test-runbook.md
git mv docs/quickstart.md docs/migration.md docs/eventsource-reference.md dev/legacy-docs/
```

- [ ] **Step 2: Write the Node toolchain files**

`website/.nvmrc`:
```
24
```

`website/package.json`:
```json
{
  "name": "onmsctl-docs",
  "private": true,
  "scripts": {
    "start": "docusaurus start",
    "build": "docusaurus build",
    "serve": "docusaurus serve",
    "clear": "docusaurus clear"
  },
  "dependencies": {
    "@docusaurus/core": "3.10.2",
    "@docusaurus/preset-classic": "3.10.2",
    "@easyops-cn/docusaurus-search-local": "0.55.3",
    "@mdx-js/react": "3.1.1",
    "clsx": "2.1.1",
    "prism-react-renderer": "2.4.1",
    "raw-loader": "4.0.2",
    "react": "19.3.0",
    "react-dom": "19.3.0"
  },
  "devDependencies": {
    "@docusaurus/module-type-aliases": "3.10.2",
    "@docusaurus/tsconfig": "3.10.2",
    "@docusaurus/types": "3.10.2",
    "typescript": "5.9.3"
  },
  "engines": {
    "node": ">=24"
  }
}
```

`website/tsconfig.json`:
```json
{
  "extends": "@docusaurus/tsconfig",
  "compilerOptions": {
    "baseUrl": "."
  },
  "exclude": [".docusaurus", "build"]
}
```

`website/static/CNAME`:
```
onmsctl.no42.org
```

- [ ] **Step 3: Write the site config (version field comes in Task 2)**

`website/docusaurus.config.ts`:
```ts
/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import {themes as prismThemes} from 'prism-react-renderer';

const config: Config = {
  title: 'onmsctl',
  tagline: 'A kubectl-style CLI for OpenNMS Horizon',
  url: 'https://onmsctl.no42.org',
  baseUrl: '/',
  organizationName: 'no42-org',
  projectName: 'onmsctl',
  trailingSlash: false,

  onBrokenLinks: 'throw',
  onBrokenAnchors: 'throw',
  markdown: {
    // .md stays CommonMark so migrated prose with <placeholders> parses;
    // only .mdx pages (imports, components) go through MDX.
    format: 'detect',
    hooks: {onBrokenMarkdownLinks: 'throw'},
  },

  i18n: {defaultLocale: 'en', locales: ['en']},

  presets: [
    [
      'classic',
      {
        docs: {
          path: '../docs',
          routeBasePath: '/',
          sidebarPath: './sidebars.ts',
        },
        blog: false,
        theme: {customCss: './src/css/custom.css'},
      } satisfies Preset.Options,
    ],
  ],

  themes: [
    [
      '@easyops-cn/docusaurus-search-local',
      {hashed: true, docsRouteBasePath: '/', indexBlog: false},
    ],
  ],

  themeConfig: {
    colorMode: {respectPrefersColorScheme: true},
    navbar: {
      title: 'onmsctl',
      items: [
        {type: 'docSidebar', sidebarId: 'docs', position: 'left', label: 'Docs'},
        {href: 'https://github.com/no42-org/onmsctl', label: 'GitHub', position: 'right'},
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Project',
          items: [
            {label: 'GitHub', href: 'https://github.com/no42-org/onmsctl'},
            {label: 'Releases', href: 'https://github.com/no42-org/onmsctl/releases'},
            {label: 'Contributing', href: 'https://github.com/no42-org/onmsctl/blob/main/CONTRIBUTING.md'},
            {label: 'Security policy', href: 'https://github.com/no42-org/onmsctl/blob/main/SECURITY.md'},
          ],
        },
      ],
      copyright:
        'Made with ❤️ in Heilbronn, Germany, Europe 🇪🇺 by <a href="https://blog.no42.org/page/about/">Ronny Trommer</a> · <a href="https://ko-fi.com/indigo423">buy me a coffee</a>',
    },
    prism: {theme: prismThemes.github, darkTheme: prismThemes.dracula},
  } satisfies Preset.ThemeConfig,
};

export default config;
```

`website/sidebars.ts`:
```ts
/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docs: ['intro'],
};

export default sidebars;
```

`website/src/css/custom.css`:
```css
/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */
```

- [ ] **Step 4: Write the landing page from the README pitch**

`docs/intro.md`: front matter `title: onmsctl`, `slug: /`, `description:` one sentence. Body: README lines 9-16 (pitch, drop the `[horizon]` reference-style link in favour of an inline link) and lines 21-23 (pre-stability notice as a `:::caution` admonition). One sentence per line, no em-dashes.

- [ ] **Step 5: Makefile targets and gitignore**

Add `docs docs-serve` to the `.PHONY` line. Add after the `docker` target:

```make
# The Docusaurus site in website/ renders the Markdown in docs/. Broken
# links and anchors fail the build. `npm test` covers the release-version
# picker behind the landing-page badge.
docs:  ## Build the documentation site into website/build (fails on broken links)
	cd website && npm ci && npm run build

docs-serve:  ## Serve the documentation site locally with live reload
	cd website && npm ci && npm start
```

Append to `.gitignore`:
```
# Docs site build output (website/)
website/node_modules/
website/build/
website/.docusaurus/
```

- [ ] **Step 6: Generate the lockfile and build**

Run: `cd website && npm install && cd .. && make docs`
Expected: `[SUCCESS] Generated static files in "build".` and `website/build/index.html` exists. `website/build/CNAME` contains `onmsctl.no42.org`.

- [ ] **Step 7: Probe that a doc outside `website/` can import a repo file**

This is the riskiest loader path (Task 4 depends on it). Create a scratch `docs/probe.mdx`:
```mdx
---
title: probe
---
import CodeBlock from '@theme/CodeBlock';
import example from '!!raw-loader!../examples/iam-user.yaml';

<CodeBlock language="yaml">{example}</CodeBlock>
```
Run: `make docs` then `grep -c 'kind' website/build/probe.html`
Expected: a count ≥ 1.

If the build fails with `Can't resolve 'raw-loader'`, add this plugin to `plugins` in `docusaurus.config.ts` (and `import path from 'node:path';` at the top), then re-run:
```ts
  plugins: [
    () => ({
      name: 'resolve-loaders-from-website',
      configureWebpack: () => ({
        resolveLoader: {modules: [path.resolve(__dirname, 'node_modules'), 'node_modules']},
      }),
    }),
  ],
```
Delete `docs/probe.mdx` once it passes.

- [ ] **Step 8: Commit**

```bash
git add website Makefile .gitignore docs dev
git commit -s -m "build(docs): scaffold Docusaurus site in website/

Assisted-by: ClaudeCode:claude-opus-5-5"
```

---

### Task 2: Latest-version resolver and landing-page badge

**Files:**
- Create: `website/src/latest-version.ts`, `website/src/latest-version.test.ts`, `website/src/components/VersionBadge.tsx`
- Modify: `website/docusaurus.config.ts`, `website/package.json`, `website/src/css/custom.css`, `website/sidebars.ts`
- Rename: `docs/intro.md` → `docs/intro.mdx`

**Interfaces:**
- Produces: `pickLatestStable(tags: readonly string[]): string | undefined` (returns `"0.4.7"`, no `v`); `latestVersion(): string` (throws when no stable tag); `siteConfig.customFields.latestVersion: string`; default export `VersionBadge` from `@site/src/components/VersionBadge`.

- [ ] **Step 1: Write the failing tests**

`website/src/latest-version.test.ts`:
```ts
/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

import {test} from 'node:test';
import assert from 'node:assert/strict';
import {pickLatestStable} from './latest-version.ts';

test('picks the highest stable tag without the v prefix', () => {
  assert.equal(pickLatestStable(['v0.4.6', 'v0.4.7', 'v0.4.5']), '0.4.7');
});

test('orders components numerically, not lexically', () => {
  assert.equal(pickLatestStable(['v0.9.9', 'v0.10.0', 'v0.2.0']), '0.10.0');
  assert.equal(pickLatestStable(['v1.0.9', 'v1.0.10']), '1.0.10');
});

test('ignores pre-release and malformed tags', () => {
  assert.equal(pickLatestStable(['v0.4.7', 'v0.5.0-rc1', 'v0.6', 'preview', 'x0.9.0']), '0.4.7');
});

test('tolerates blank lines and surrounding whitespace from git output', () => {
  assert.equal(pickLatestStable(['', '  v0.1.0  ', 'v0.0.2', '']), '0.1.0');
});

test('returns undefined when no stable tag exists', () => {
  assert.equal(pickLatestStable([]), undefined);
  assert.equal(pickLatestStable(['preview', 'v1.0.0-rc1']), undefined);
});
```

Add `"test": "node --test src/latest-version.test.ts"` to `scripts` in `website/package.json`, and change the `docs` recipe in `Makefile` to `cd website && npm ci && npm test && npm run build`.

- [ ] **Step 2: Run to verify they fail**

Run: `cd website && npm test`
Expected: FAIL, module `./latest-version.ts` not found.

- [ ] **Step 3: Implement**

`website/src/latest-version.ts`:
```ts
/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

import {execFileSync} from 'node:child_process';

// Stable releases only: v0.5.0-rc1 must never outrank v0.4.7.
const STABLE = /^v(\d+)\.(\d+)\.(\d+)$/;

/** Highest stable vX.Y.Z tag, without the leading "v". */
export function pickLatestStable(tags: readonly string[]): string | undefined {
  let best: number[] | undefined;
  for (const tag of tags) {
    const m = STABLE.exec(tag.trim());
    if (!m) continue;
    const v = m.slice(1).map(Number);
    const cmp = best ? v[0] - best[0] || v[1] - best[1] || v[2] - best[2] : 1;
    if (cmp > 0) best = v;
  }
  return best?.join('.');
}

/** Latest release from the checkout's git tags. Throws when there are none. */
export function latestVersion(): string {
  const tags = execFileSync('git', ['tag', '--list', 'v*'], {encoding: 'utf8'}).split('\n');
  const version = pickLatestStable(tags);
  if (!version) {
    throw new Error(
      'No stable vX.Y.Z git tag found. The docs build reads the latest release from tags. ' +
        'Run `git fetch --tags`, or check out with fetch-depth: 0 and fetch-tags: true.',
    );
  }
  return version;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd website && npm test`
Expected: `# pass 5`, `# fail 0`.

- [ ] **Step 5: Wire the version into the config**

In `website/docusaurus.config.ts` add `import {latestVersion} from './src/latest-version';` after the other imports, and add after `i18n`:
```ts
  // Resolved from git tags at build start; see src/latest-version.ts.
  customFields: {latestVersion: latestVersion()},
```

- [ ] **Step 6: Badge component and styles**

`website/src/components/VersionBadge.tsx`:
```tsx
/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

import React from 'react';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';

export default function VersionBadge(): React.JSX.Element {
  const {siteConfig} = useDocusaurusContext();
  const version = siteConfig.customFields.latestVersion as string;
  return (
    <a
      className="version-badge"
      href={`https://github.com/no42-org/onmsctl/releases/tag/v${version}`}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={`onmsctl v${version} release notes`}>
      v{version}
      <span className="version-badge-arrow" aria-hidden="true">↗</span>
    </a>
  );
}
```

Append to `website/src/css/custom.css`:
```css
.version-badge {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  margin-bottom: 1rem;
  padding: 0.25rem 0.75rem;
  border: 1px solid var(--ifm-color-primary-light);
  border-radius: 20px;
  background-color: var(--ifm-color-emphasis-100);
  color: var(--ifm-color-primary);
  font-size: 0.85rem;
  font-weight: 500;
  transition: border-color 0.2s, transform 0.2s;
}

.version-badge:hover {
  border-color: var(--ifm-color-primary);
  text-decoration: none;
}

.version-badge:focus-visible {
  outline: 2px solid var(--ifm-color-primary);
  outline-offset: 2px;
}

.version-badge-arrow {
  transition: transform 0.2s;
}

.version-badge:hover .version-badge-arrow {
  transform: translateX(2px);
}
```

- [ ] **Step 7: Put the badge on the landing page only**

```bash
git mv docs/intro.md docs/intro.mdx
```
Insert after the front matter of `docs/intro.mdx`:
```mdx
import VersionBadge from '@site/src/components/VersionBadge';

<VersionBadge />
```
Check the pitch prose for bare `<` or `{` (Migration rule 7).

- [ ] **Step 8: Verify the rendered badge**

Run: `make docs && grep -o 'releases/tag/v[0-9.]*' website/build/index.html | head -1`
Expected: `releases/tag/v0.4.7` (the highest stable tag in the checkout; `git tag --list 'v*' --sort=-version:refname | head -1` must agree).

Run: `grep -c 'version-badge' website/build/index.html` → ≥ 1. Task 8 Step 2 confirms no other page carries it.

- [ ] **Step 9: Verify the no-tags failure (Review Focus 2)**

```bash
tmp=$(mktemp -d)
git clone -q --depth 1 --no-tags --branch docs/docusaurus-site "file://$PWD" "$tmp"
make -C "$tmp" docs 2>&1 | grep 'No stable vX.Y.Z git tag found'
rm -rf "$tmp"
```
The clone takes committed state, so run this after Step 10.
Expected: the grep prints the error line.

- [ ] **Step 10: Commit**

```bash
git add website docs Makefile
git commit -s -m "feat(docs): show latest release badge on the landing page

Assisted-by: ClaudeCode:claude-opus-5-5"
```

---

### Task 3: Getting started and Concepts pages

**Files:**
- Create: `docs/getting-started/install.md`, `docs/getting-started/configure-context.md`, `docs/getting-started/quickstart.md`, `docs/concepts/declarative-apply.md`, `docs/concepts/read-only-contexts.md`
- Modify: `website/sidebars.ts`

**Interfaces:**
- Consumes: sidebar id `docs` (Task 1).
- Produces: doc ids `getting-started/install`, `getting-started/configure-context`, `getting-started/quickstart`, `concepts/declarative-apply`, `concepts/read-only-contexts`.

- [ ] **Step 1: Write the pages** (apply Migration rules 1-8)

| Page | Sources |
|---|---|
| `getting-started/install.md` | QS §1 (39-48) as "Prerequisites"; README Install (42-96), merged with QS §2 (49-102), keep the fuller copy; README Build from source (97-123); README Container image (124-185) |
| `getting-started/configure-context.md` | README Configure a context (186-227) merged with QS §3 (103-173) |
| `getting-started/quickstart.md` | QS intro (1-17, minus the Contents list 18-38), QS §4 Core concepts (174-210), QS §5 Five-minute tour (211-235). End with a "Next steps" list linking to `../kinds/event-source.mdx` etc. **Add those links in Task 4**, once the targets exist. |
| `concepts/declarative-apply.md` | README Declarative apply (228-284). The link to migration at README 281 targets `../guides/migration.md#removed-imperative-verbs--onmsctl-apply--f`; add it in Task 5 when that page exists. Until then leave it as plain text. |
| `concepts/read-only-contexts.md` | README Verb aliases & read-only contexts (597-607) |

- [ ] **Step 2: Add them to the sidebar**

`website/sidebars.ts`, `docs` becomes:
```ts
  docs: [
    'intro',
    {
      type: 'category',
      label: 'Getting started',
      collapsed: false,
      items: ['getting-started/install', 'getting-started/configure-context', 'getting-started/quickstart'],
    },
    {
      type: 'category',
      label: 'Concepts',
      items: ['concepts/declarative-apply', 'concepts/read-only-contexts'],
    },
  ],
```

- [ ] **Step 3: Build**

Run: `make docs`
Expected: success. A broken link or anchor error names the file; fix it and re-run.

- [ ] **Step 4: Commit**

```bash
git add docs website/sidebars.ts
git commit -s -m "docs: add getting-started and concepts pages

Assisted-by: ClaudeCode:claude-opus-5-5"
```

---

### Task 4: Kind pages

**Files:**
- Create: `docs/kinds/event-source.mdx`, `requisition.mdx`, `user.mdx`, `snmp-config.mdx`, `maintenance.mdx`, `datacollection-source.mdx`, `business-service.mdx`
- Modify: `website/sidebars.ts`, `docs/getting-started/quickstart.md`

**Interfaces:**
- Consumes: raw-loader import path proven in Task 1 Step 7.
- Produces: doc ids `kinds/<name>`.

- [ ] **Step 1: Write each page** with this section order: intro (what the kind manages), `## YAML`, `## Apply`, `## Inspect and delete`, `## Convert` (only where a `convert` verb exists), `## Example`. Put version-gate notes (Trapd, data collection) in a `:::note`.

| Page | Sources | Example file(s) |
|---|---|---|
| `event-source.mdx` | README 285-314, QS §6 (236-290), all of `dev/legacy-docs/eventsource-reference.md` as `## Convert findings`, `## Field semantics`, `## Apply-time limitations` | `event-source-minimal.yaml`, `event-source-full.yaml` |
| `requisition.mdx` | README 315-352, QS §7 (291-364) | `requisition-acme-prod.yaml` |
| `user.mdx` | README 353-388, QS §8 (365-408) | `iam-user.yaml` |
| `snmp-config.mdx` | README 389-422, QS §9 (409-440) | `snmp-config.yaml` |
| `maintenance.mdx` | README 423-477, QS §9c (467-488) | `maintenance.yaml` |
| `datacollection-source.mdx` | README 478-506, QS §9b (441-466) | `datacollection-source.yaml` |
| `business-service.mdx` | README 507-573, QS §9d (489-513) | `business-service.yaml` |

The `## Example` section embeds the file from disk. Example for `requisition.mdx` (path is relative to `docs/kinds/`):
```mdx
import CodeBlock from '@theme/CodeBlock';
import example from '!!raw-loader!../../examples/requisition-acme-prod.yaml';

<CodeBlock language="yaml" title="examples/requisition-acme-prod.yaml">{example}</CodeBlock>
```
Put the `import` lines directly after the front matter. For `event-source.mdx` import two files as `minimal` and `full`.

Migration links (README 348-349, 384-385): leave as plain text now; Task 5 turns them into links to `../guides/migration.md#...`.

- [ ] **Step 2: Sidebar and Next steps**

Append to `docs` in `website/sidebars.ts`:
```ts
    {
      type: 'category',
      label: 'Kinds',
      items: [
        'kinds/event-source',
        'kinds/requisition',
        'kinds/user',
        'kinds/snmp-config',
        'kinds/maintenance',
        'kinds/datacollection-source',
        'kinds/business-service',
      ],
    },
```
Add the "Next steps" list to `docs/getting-started/quickstart.md`, one link per kind page, e.g. `- [Event configuration](../kinds/event-source.mdx)`.

- [ ] **Step 3: Build and check the embedded examples**

Run: `make docs && grep -c 'acme-prod' website/build/kinds/requisition.html`
Expected: build succeeds; count ≥ 1. An MDX parse error names file and line; apply Migration rule 7.

- [ ] **Step 4: Commit**

```bash
git add docs website/sidebars.ts
git commit -s -m "docs: add one page per kind with embedded examples

Assisted-by: ClaudeCode:claude-opus-5-5"
```

---

### Task 5: Guides and Reference pages, footer docs links, remove legacy files

**Files:**
- Create: `docs/guides/migration.md`, `docs/guides/troubleshooting.md`, `docs/reference/global-flags.md`, `output-formats.md`, `exit-codes.md`, `shell-completions.md`, `tls.md`, `editor-integration.md`, `compatibility.md`
- Modify: `website/sidebars.ts`, `website/docusaurus.config.ts`, `docs/concepts/declarative-apply.md`, `docs/kinds/requisition.mdx`, `docs/kinds/user.mdx`
- Delete: `dev/legacy-docs/`

**Interfaces:**
- Produces: doc ids `guides/*`, `reference/*`; anchors `guides/migration.md#removed-imperative-verbs--onmsctl-apply--f`, `#provisionpl-verb--onmsctl`, `#legacy-usersxml--onmsctl` (keep the source headings unchanged so these anchors stay stable).

- [ ] **Step 1: Write the pages**

| Page | Sources |
|---|---|
| `guides/migration.md` | all of `dev/legacy-docs/migration.md`; keep its `##`/`###` heading text as is |
| `guides/troubleshooting.md` | QS §13 (587-617), minus the trailing links to README/examples/schemas/runbook (runbook is contributor material) |
| `reference/global-flags.md` | QS §10 (514-542) |
| `reference/output-formats.md` | README 593-596 + the output-format half of QS §11 (543-571) |
| `reference/exit-codes.md` | README 608-622 + the exit-code half of QS §11 |
| `reference/shell-completions.md` | README 623-635 + QS §12 (572-586) |
| `reference/tls.md` | README 636-642 |
| `reference/editor-integration.md` | README 576-592; schema URLs stay raw.githubusercontent.com |
| `reference/compatibility.md` | README 643-666 |

- [ ] **Step 2: Turn the deferred migration mentions into links**

In `concepts/declarative-apply.md`, `kinds/requisition.mdx`, `kinds/user.mdx`, link the plain-text migration mentions left in Tasks 3-4 to the anchors listed under Interfaces.

- [ ] **Step 3: Sidebar and footer**

Append to `docs` in `website/sidebars.ts`:
```ts
    {
      type: 'category',
      label: 'Guides',
      items: ['guides/migration', 'guides/troubleshooting'],
    },
    {
      type: 'category',
      label: 'Reference',
      items: [
        'reference/global-flags',
        'reference/output-formats',
        'reference/exit-codes',
        'reference/shell-completions',
        'reference/tls',
        'reference/editor-integration',
        'reference/compatibility',
      ],
    },
```
In `website/docusaurus.config.ts`, insert as the first entry of `footer.links`:
```ts
        {
          title: 'Docs',
          items: [
            {label: 'Getting started', to: '/getting-started/install'},
            {label: 'Kinds', to: '/kinds/event-source'},
            {label: 'Reference', to: '/reference/global-flags'},
          ],
        },
```

- [ ] **Step 4: Coverage check, then delete the legacy files**

For every `##` and `###` heading in `README.md` (lines 42-666) and in each `dev/legacy-docs/*.md`, confirm the content is on a page. List them:
```bash
grep -n '^##' README.md dev/legacy-docs/*.md
```
Tick each against the Source map. Then:
```bash
git rm -r dev/legacy-docs
```

- [ ] **Step 5: Build**

Run: `make docs`
Expected: success.

- [ ] **Step 6: Commit**

```bash
git add docs website dev
git commit -s -m "docs: add guides and reference pages, retire flat docs files

Assisted-by: ClaudeCode:claude-opus-5-5"
```

---

### Task 6: Slim README and repoint in-repo links

**Files:**
- Create: `scripts/check-site-urls.sh`
- Modify: `README.md`, `SUPPORT.md`, `SECURITY.md:77`, `examples/README.md:9-18`, `.github/ISSUE_TEMPLATE/question.yml:10`, `CONTRIBUTING.md`, `AGENTS.md`, `Makefile`

**Interfaces:**
- Produces: `make docs-urls` runs `scripts/check-site-urls.sh` after `make docs`.

- [ ] **Step 1: Write the URL check (fails first)**

`scripts/check-site-urls.sh`:
```bash
#!/usr/bin/env bash
# Copyright 2026 Ronny Trommer <ronny@no42.org>
# SPDX-License-Identifier: Apache-2.0
#
# Every https://onmsctl.no42.org/<path> link in tracked files must map to a
# page in website/build. Docusaurus checks links inside the site; this
# checks the links pointing into it from README, SUPPORT, templates, etc.
set -euo pipefail

build=website/build
test -d "$build" || { echo "run 'make docs' first" >&2; exit 1; }

fail=0
while read -r url; do
  path=${url#https://onmsctl.no42.org}
  path=${path%%#*}
  path=${path%/}
  if [[ -z "$path" ]]; then file="$build/index.html"; else file="$build$path.html"; fi
  [[ -f "$file" || -f "$build$path/index.html" ]] || { echo "dead site link: $url" >&2; fail=1; }
done < <(git grep -hoE 'https://onmsctl\.no42\.org[^) "'"'"'>]*' -- ':!website' ':!dev' | sort -u)
exit $fail
```
`chmod +x scripts/check-site-urls.sh`. Add to `Makefile` (and `.PHONY`):
```make
docs-urls: docs  ## Check links from repo files into the docs site resolve
	scripts/check-site-urls.sh
```
Add one deliberate dead link to `SUPPORT.md` (`https://onmsctl.no42.org/nope`), run `make docs-urls`, expect `dead site link: https://onmsctl.no42.org/nope` and exit 1. Remove it.

Note: with `trailingSlash: false`, Docusaurus writes `build/<path>.html`; with `undefined` it writes `<path>/index.html`. The script accepts both.

- [ ] **Step 2: Rewrite README.md (about 60 lines)**

Keep, in order: the badges (lines 3-7); the pitch (lines 9-17) with `docs/quickstart.md` replaced by `https://onmsctl.no42.org/getting-started/quickstart`; the pre-stability notice; `## Install` with only the one-liner install block from README 42-96 plus "Other install methods: https://onmsctl.no42.org/getting-started/install"; `## Example` with one short `apply -f` block taken from README 228-284; `## Documentation` with links to Getting started, Kinds (`/kinds/event-source`), Reference (`/reference/global-flags`), Migration (`/guides/migration`); `## License` and `## Contributing` (lines 667-676) unchanged. One sentence per line, no em-dashes in new prose.

- [ ] **Step 3: Repoint the other files**

| File | Change |
|---|---|
| `SUPPORT.md:5-8` | Quick Start → `https://onmsctl.no42.org/getting-started/quickstart`; EventSource reference → `https://onmsctl.no42.org/kinds/event-source`; Migration → `https://onmsctl.no42.org/guides/migration` |
| `SECURITY.md:77` | `README.md#install` → `https://onmsctl.no42.org/getting-started/install` |
| `examples/README.md:9-18` | Docs column: one link per row to its kind page, e.g. `[docs](https://onmsctl.no42.org/kinds/requisition)` |
| `.github/ISSUE_TEMPLATE/question.yml:10` | Quick Start URL → `https://onmsctl.no42.org/getting-started/quickstart` |
| `CONTRIBUTING.md` | Add a `## Documentation` section: site source in `docs/`, app in `website/`, `make docs` / `make docs-serve`, broken links fail the build; link `dev/manual-test-runbook.md` for manual verification |
| `AGENTS.md` | Add rows to the command table: `make docs` (build the site, fails on broken links) and `make docs-serve` |

- [ ] **Step 4: Verify**

Run: `make docs-urls && git grep -nE 'docs/(quickstart|migration|eventsource-reference|manual-test-runbook)\.md|README\.md#' -- ':!dev'`
Expected: `make docs-urls` exits 0; the grep prints nothing.

- [ ] **Step 5: Commit**

```bash
git add README.md SUPPORT.md SECURITY.md examples/README.md .github/ISSUE_TEMPLATE/question.yml CONTRIBUTING.md AGENTS.md Makefile scripts/check-site-urls.sh
git commit -s -m "docs: slim README to a landing page and point links at the site

Assisted-by: ClaudeCode:claude-opus-5-5"
```

---

### Task 7: CI gate, Pages deploy, Dependabot

**Files:**
- Modify: `.github/workflows/gates.yml`, `.github/dependabot.yml`
- Create: `.github/workflows/docs.yml`

- [ ] **Step 1: Add the docs job to the shared gate**

Append under `jobs:` in `.github/workflows/gates.yml`:
```yaml
  docs:
    # The Docusaurus site in website/. Separate from `verify` so a broken
    # link is not reported as a Rust failure. No npm cache: this gate also
    # runs for release tags, and a restored cache there is a poisoning path.
    runs-on: ubuntu-24.04
    permissions:
      contents: read
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          # The landing-page badge reads the latest release from v* tags.
          fetch-depth: 0
          fetch-tags: true
          persist-credentials: false

      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0
        with:
          node-version-file: website/.nvmrc

      - name: Build docs and check links into the site
        run: make docs-urls
```

- [ ] **Step 2: Add the deploy workflow**

`.github/workflows/docs.yml`:
```yaml
# Publishes the Docusaurus site to GitHub Pages (onmsctl.no42.org).
# The gate (gates.yml `docs` job) already proves the site builds on every
# PR; this workflow only deploys from main and on release tags.
name: docs

on:
  push:
    branches: [main]
    # Release tags redeploy so the landing-page badge shows the new version.
    # Path filters are not evaluated for tag pushes.
    tags: ['v*']
    paths:
      - 'docs/**'
      - 'website/**'
      - 'examples/**'
      - '.github/workflows/docs.yml'
  workflow_dispatch:

permissions:
  contents: read

# A release tag and a docs push can land together. Serialize deployments
# to the single github-pages environment instead of failing one.
concurrency:
  group: pages
  cancel-in-progress: false

jobs:
  build:
    runs-on: ubuntu-24.04
    permissions:
      contents: read
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
          fetch-tags: true
          persist-credentials: false

      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0
        with:
          node-version-file: website/.nvmrc

      - name: Build docs
        run: make docs

      - uses: actions/configure-pages@45bfe0192ca1faeb007ade9deae92b16b8254a0d # v6.0.0

      - uses: actions/upload-pages-artifact@fc324d3547104276b827a68afc52ff2a11cc49c9 # v5.0.0
        with:
          path: website/build

  deploy:
    needs: build
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    permissions:
      pages: write
      id-token: write
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - name: Deploy to GitHub Pages
        id: deployment
        uses: actions/deploy-pages@368f82528645a54fb793d4d04e342629a3f51346 # v5.0.1
```

- [ ] **Step 3: Dependabot npm entry**

Append to `updates:` in `.github/dependabot.yml`:
```yaml
  # The docs site (website/) has its own npm lockfile.
  - package-ecosystem: npm
    directory: /website
    schedule:
      interval: weekly
    open-pull-requests-limit: 5
    commit-message:
      prefix: build
      include: scope
    labels:
      - dependencies
    groups:
      docs-deps:
        patterns:
          - "*"
        update-types:
          - minor
          - patch
```

- [ ] **Step 4: Lint**

Run: `make lint-actions`
Expected: actionlint and zizmor report no findings. If zizmor flags something, fix the workflow; do not add ignores without a comment explaining why.

- [ ] **Step 5: Commit**

```bash
git add .github
git commit -s -m "ci(docs): gate the docs build and deploy the site to Pages

Assisted-by: ClaudeCode:claude-opus-5-5"
```

---

### Task 8: Full verification and PR

- [ ] **Step 1: Full gates**

Run: `make verify && make lint-actions && make docs-urls`
Expected: all three exit 0.

- [ ] **Step 2: Badge only on the landing page**

Run: `grep -rl 'class="version-badge"' website/build --include=*.html`
Expected: exactly `website/build/index.html`.

- [ ] **Step 3: Manual look**

Run: `make docs-serve`, open `http://localhost:3000`. Check: badge shows `v0.4.7 ↗` and links to the release; light and dark mode both readable; search finds "requisition"; footer shows the heart line with both links; every sidebar entry opens.

- [ ] **Step 4: Push and open the PR**

```bash
git push -u origin docs/docusaurus-site
gh pr create --title "docs: move documentation to a Docusaurus site" --body-file - <<'EOF'
Moves the README and docs/ into a Docusaurus site served from https://onmsctl.no42.org.

Spec: dev/specs/2026-09-25-docusaurus-site-design.md

After merge, the maintainer needs to:
- Settings, Pages: source "GitHub Actions".
- DNS: onmsctl.no42.org CNAME no42-org.github.io.
- Settings, Pages: custom domain onmsctl.no42.org, then Enforce HTTPS.
- Repo homepage URL: https://onmsctl.no42.org.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
```
Expected: `gate / docs` passes on the PR.

- [ ] **Step 5: After merge and the manual steps**

`curl -sI https://onmsctl.no42.org | head -1` → `HTTP/2 200`. Push of the next `v*` tag triggers `docs.yml` and the badge updates (Review Focus 5).
