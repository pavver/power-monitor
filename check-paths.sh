#!/usr/bin/env bash
# Fails when the repository depends on one machine: an absolute path to a
# home, a temp folder of a tool or a file:// URL in a tracked file, or a
# Cargo path dependency outside the repository (the design system and
# other crates come from git, never from a neighbouring checkout).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"
status=0

# Home folders (Linux, macOS, Windows), tool scratch folders, file URLs,
# and "~/" paths to project checkouts.
LOCAL='(/home/[A-Za-z0-9_.-]+/|/Users/[A-Za-z0-9_.-]+/|[A-Za-z]:\\Users\\|/tmp/claude|file://|~/(rust|src|projects|code)/)'
if hits=$(git ls-files -z | grep -zv -e '^check-paths\.sh$' -e '^firmware/vendor/' \
    | xargs -0 grep -nIE "$LOCAL" 2>/dev/null); then
  echo "Local paths in tracked files:"
  echo "$hits"
  status=1
fi

# Every `path = "…"` of a Cargo.toml must stay inside the repository.
while IFS= read -r manifest; do
  dir=$(dirname "$manifest")
  while IFS= read -r dep; do
    target=$(realpath -m "$dir/$dep")
    case "$target/" in
      "$ROOT"/*) ;;
      *) echo "$manifest: path dependency \"$dep\" is outside the repository"; status=1 ;;
    esac
  done < <(grep -oE 'path *= *"[^"]+"' "$manifest" | sed -E 's/.*"([^"]+)"/\1/')
done < <(git ls-files '*Cargo.toml' | grep -v '^firmware/vendor/')

exit $status
