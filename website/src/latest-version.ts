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
