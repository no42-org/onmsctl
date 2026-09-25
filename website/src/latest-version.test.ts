/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

import {test} from 'node:test';
import assert from 'node:assert/strict';
import {normalizeVersionOverride, pickLatestStable} from './latest-version.ts';

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

test('version override accepts a tag or a bare version', () => {
  assert.equal(normalizeVersionOverride('v0.4.7'), '0.4.7');
  assert.equal(normalizeVersionOverride('0.4.7'), '0.4.7');
  assert.equal(normalizeVersionOverride(' v0.10.0\n'), '0.10.0');
});

test('version override rejects anything but a stable version', () => {
  for (const bad of ['', 'latest', 'v0.5.0-rc1', 'v0.4', 'vv0.4.7', '0.4.7.1']) {
    assert.throws(() => normalizeVersionOverride(bad), /ONMSCTL_DOCS_VERSION/, bad);
  }
});
