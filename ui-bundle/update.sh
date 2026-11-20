#!/usr/bin/env bash
# Moves the device page to another commit of the design system:
#   ui-bundle/update.sh <rev>     (a commit of github.com/pavver/operator-design-system)
# Rewrites the rev in Cargo.toml, rebuilds the operator-ui part of
# firmware/ui/index.html (styles, markup, icons, timings) and runs the
# page check when PLAYWRIGHT_CORE points at playwright-core.
# The page's script ports the components' behaviour by hand: if a
# component's API or behaviour changed in that commit, the check fails
# and the script needs the same change.
set -euo pipefail
REV="${1:?usage: ui-bundle/update.sh <design system commit>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$HERE")"

sed -i -E "s|(operator-design-system\", rev = \")[^\"]+|\1$REV|" "$HERE/Cargo.toml"
(cd "$HERE" && cargo update --quiet -p operator-ui && cargo test --quiet && cargo run --quiet)
python3 "$ROOT/firmware/ui/make-font.py"

if [ -n "${PLAYWRIGHT_CORE:-}" ]; then
  out="$(mktemp -d)"
  node "$ROOT/firmware/ui/ui-check.js" "$ROOT/firmware/ui/index.html" "$out"
  echo "screenshots: $out"
else
  echo "PLAYWRIGHT_CORE not set: run firmware/ui/ui-check.js yourself"
fi
git -C "$ROOT" diff --stat
