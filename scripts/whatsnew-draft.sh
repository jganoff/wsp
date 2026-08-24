#!/usr/bin/env bash
# Collect the whatsnew blocks merged since a tag into a draft release note.
#
#   scripts/whatsnew-draft.sh            # since the most recent tag
#   scripts/whatsnew-draft.sh v0.19.0    # since a specific tag
#
# Squash is the only merge method in this repo and the squash body is the PR
# description verbatim, so every merged PR's whatsnew block is already in the
# commit body on main. No network, no bot, no fragment files to clean up.
#
# The output is raw material, not the finished section. Ordering by impact,
# merging several symptoms of one cause into a single line, and deciding what
# deserves prose are judgements you can only make with all of them in view.
set -euo pipefail

since="${1:-}"
if [ -z "$since" ]; then
  since=$(git describe --tags --abbrev=0 2>/dev/null || true)
fi
if [ -z "$since" ]; then
  echo "no tag found; pass one explicitly: scripts/whatsnew-draft.sh v0.19.0" >&2
  exit 1
fi

range="$since..HEAD"
count=$(git rev-list --count "$range")
echo "# collected from $count commit(s) in $range" >&2
echo >&2

# %B is the raw body; the record separator keeps multi-line notes together.
notes=$(git log "$range" --format='%B%x00' | awk '
  BEGIN { RS = "\0" }
  {
    inblock = 0
    n = split($0, lines, "\n")
    for (i = 1; i <= n; i++) {
      line = lines[i]
      sub(/\r$/, "", line)
      if (line ~ /^```whatsnew[[:space:]]*$/) { inblock = 1; continue }
      if (inblock && line ~ /^```/)           { inblock = 0; continue }
      if (inblock)                            { print line }
    }
  }
')

# Drop NONE markers and blank lines left behind by them.
cleaned=$(printf '%s\n' "$notes" \
  | grep -v -i -x '[[:space:]]*none[[:space:]]*' \
  | sed '/^[[:space:]]*$/d')

if [ -z "$cleaned" ]; then
  echo "no user-facing notes in $range" >&2
  exit 0
fi

printf '%s\n' "$cleaned"
