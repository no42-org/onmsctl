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

/**
 * Validate an ONMSCTL_DOCS_VERSION value ("v0.4.7" or "0.4.7").
 * Returns the version without the leading "v". Throws on anything else.
 */
export function normalizeVersionOverride(value: string): string {
  const trimmed = value.trim();
  const tag = trimmed.startsWith('v') ? trimmed : `v${trimmed}`;
  if (!STABLE.test(tag)) {
    throw new Error(
      `ONMSCTL_DOCS_VERSION must be a stable release like v0.4.7 or 0.4.7, got "${value}".`,
    );
  }
  return tag.slice(1);
}

/**
 * Latest release for the landing-page badge.
 * ONMSCTL_DOCS_VERSION wins when set: the deploy workflow puts the latest
 * published GitHub Release there, because tags also exist for draft releases.
 * Otherwise the highest stable git tag. Throws when neither yields a version.
 */
export function latestVersion(): string {
  const override = process.env.ONMSCTL_DOCS_VERSION;
  if (override !== undefined) return normalizeVersionOverride(override);
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
