/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import {themes as prismThemes} from 'prism-react-renderer';
import {latestVersion} from './src/latest-version';

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

  // Resolved from git tags at build start; see src/latest-version.ts.
  customFields: {latestVersion: latestVersion()},

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
