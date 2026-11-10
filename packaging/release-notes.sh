#!/usr/bin/env bash
# Writes release notes to stdout: the tag annotation (if the ref is an annotated tag) followed by
# the commit log since the previous tag.
#   packaging/release-notes.sh <tag-or-ref>
set -euo pipefail
ref=${1:?usage: release-notes.sh <tag-or-ref>}

if git cat-file -e "refs/tags/$ref" 2>/dev/null && [ "$(git cat-file -t "refs/tags/$ref")" = tag ]; then
  git tag -l --format='%(contents)' "$ref" | sed '/^-----BEGIN PGP SIGNATURE-----/,$d'
  echo
fi

prev=$(git describe --tags --abbrev=0 --match 'v*' "$ref^" 2>/dev/null || true)
echo "## Changes"
echo
if [ -n "$prev" ]; then
  echo "Since $prev:"
  echo
  range="$prev..$ref"
else
  range="$ref"
fi
git log --no-merges --pretty='- %s (%h)' "$range"
echo
cat <<'NOTE'
## Installing

- **Linux**: extract the `.tar.gz` and run `./install.sh`, or make the `.AppImage` executable.
- **macOS**: unzip and drag `Quarry.app` to Applications. The app is ad-hoc signed but not
  notarised: on first launch right-click the app and choose Open (see the user guide).
- **Windows**: unzip and run `quarry.exe`.

Verify downloads against `SHA256SUMS`.
NOTE
