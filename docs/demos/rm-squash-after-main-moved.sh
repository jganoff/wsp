#!/usr/bin/env bash
# Demo: `wsp rm` on a workspace whose branch was squash-merged after the
# default branch had already changed the same file nearby.
#
#   WSP=./target/release/wsp docs/demos/rm-squash-after-main-moved.sh --record out.cast
#
# Setup runs silently inside a throwaway HOME; only `wsp rm` is shown.
set -euo pipefail

if [ "${1:-}" = "--show" ]; then
  printf '$ wsp rm demo\n'
  sleep 1
  "$WSP" rm demo
  sleep 2
  exit 0
fi

SELF=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")
OUT=${2:-}
[ -z "$OUT" ] || OUT=$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")
WSP=$(cd "$(dirname "${WSP:?set WSP to the wsp binary}")" && pwd)/$(basename "$WSP")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HOME="$tmp/home" XDG_DATA_HOME="$tmp/home/.local/share" WSP
mkdir -p "$HOME"
git config --global user.email demo@example.com
git config --global user.name demo
git config --global commit.gpgsign false
git config --global init.defaultBranch main
git config --global url."$tmp/src/".insteadOf "git@test.local:user/"

src="$tmp/src/alpha"
git init -q "$src"
seq 1 10 | sed 's/^/line /' >"$src/f.txt"
git -C "$src" add f.txt
git -C "$src" commit -q -m "ten lines"
"$WSP" registry add git@test.local:user/alpha >/dev/null 2>&1
"$WSP" new demo alpha >/dev/null 2>&1

# The workspace branch edits line 4.
ws=$(find "$HOME" -maxdepth 4 -type d -path '*/demo/alpha' | head -1)
sed -i 's/^line 4$/line 4 (feature)/' "$ws/f.txt"
git -C "$ws" commit -q -am "feature: edit line 4"
git -C "$ws" push -q origin HEAD:refs/heads/demo 2>/dev/null

# main edits line 6 first, then squash-merges the workspace branch.
sed -i 's/^line 6$/line 6 (main)/' "$src/f.txt"
git -C "$src" commit -q -am "main: edit line 6"
git -C "$src" merge -q --squash demo
git -C "$src" commit -q -m "squash-merge demo"

cd "$tmp"
if [ "${1:-}" = "--record" ]; then
  asciinema rec --quiet --overwrite --cols 100 --rows 12 -c "bash $SELF --show" "$OUT"
else
  bash "$SELF" --show
fi
