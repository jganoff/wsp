#!/usr/bin/env bash
# Demo: `wsp exec` when started from a process that has GIT_DIR set, as git does
# for hooks and aliases. Each repo must answer for itself.
#
#   WSP=./target/release/wsp docs/demos/exec-inherited-git-dir.sh --record out.cast
#
# Setup runs silently inside a throwaway HOME; only the final command is shown.
set -euo pipefail

if [ "${1:-}" = "--show" ]; then
  printf '$ GIT_DIR=<alpha>/.git wsp exec demo -- git log -1 --format=%%s\n'
  sleep 1
  GIT_DIR="$HOOK_GIT_DIR" "$WSP" exec demo -- git log -1 --format=%s
  sleep 2
  exit 0
fi

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
for r in alpha beta; do
  git init -q "$tmp/src/$r"
  git -C "$tmp/src/$r" commit -q --allow-empty -m "init $r"
  "$WSP" registry add "git@test.local:user/$r" >/dev/null
done
"$WSP" new demo alpha beta >/dev/null 2>&1
export HOOK_GIT_DIR="$tmp/src/alpha/.git"

cd "$tmp"
if [ "${1:-}" = "--record" ]; then
  asciinema rec --quiet --overwrite --cols 80 --rows 10 -c "bash $(cd "$OLDPWD" && pwd)/$0 --show" "$(cd "$OLDPWD" && pwd)/${2:?cast path}"
else
  bash "$(cd "$OLDPWD" && pwd)/$0" --show
fi
