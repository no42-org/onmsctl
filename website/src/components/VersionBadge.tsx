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
