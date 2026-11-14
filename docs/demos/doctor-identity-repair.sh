#!/usr/bin/env bash
# Demo: a clone's origin is repointed at a different repository. `wsp sync`
# refuses it, and `wsp doctor --fix` records the identity origin now names.
#
#   WSP=./target/release/wsp docs/demos/doctor-identity-repair.sh
#
# Setup runs silently inside a throwaway HOME; only the final commands are shown.
set -euo pipefail

if [ "${1:-}" = "--show" ]; then
  printf '$ git -C alpha remote set-url origin git@test.local:user/gamma\n'
  git -C alpha remote set-url origin git@test.local:user/gamma
  printf '$ wsp sync\n'
  "$WSP" sync 2>&1 | sed "s|$PWD|<workspace>|g" || true
  printf '$ wsp doctor --fix\n'
  "$WSP" doctor --fix 2>&1 | grep -E 'alpha|fix' || true
  printf '$ wsp sync\n'
  "$WSP" sync 2>&1 | sed "s|$PWD|<workspace>|g" || true
  exit 0
fi

WSP=$(cd "$(dirname "${WSP:?set WSP to the wsp binary}")" && pwd)/$(basename "$WSP")
self=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")
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
# gamma is alpha's repository under a new name.
git clone -q --bare "$tmp/src/alpha" "$tmp/src/gamma"
"$WSP" new demo alpha beta >/dev/null 2>&1
cd "$HOME/dev/workspaces/demo"
bash "$self" --show
