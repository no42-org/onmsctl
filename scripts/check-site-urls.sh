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
done < <(git grep -hoE 'https://onmsctl\.no42\.org[^) "'"'"'>`]*' -- ':!website' ':!dev' | sort -u)
exit $fail
