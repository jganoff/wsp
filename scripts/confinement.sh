#!/usr/bin/env bash
# Verify macOS workspace confinement against a real release binary.
#
# The child has a distinct macOS principal. Only the fixture workspace and
# copied binary are owned by that principal; the global store and sibling
# canaries remain private to the runner. This proves the authority boundary
# that a workspace-local invocation must respect. CI calls this only on
# macOS, and missing account-management support is a failure.
set -euo pipefail

WSP=""
while [ $# -gt 0 ]; do
    case "$1" in
        --wsp) WSP="$2"; shift 2 ;;
        *) echo "usage: $0 --wsp <path>" >&2; exit 2 ;;
    esac
done

[ "$(uname -s)" = "Darwin" ] || {
    echo "macOS POSIX confinement requires Darwin" >&2
    exit 2
}
[ -n "$WSP" ] || { echo "usage: $0 --wsp <path>" >&2; exit 2; }
case "$WSP" in
    */*) WSP="$(cd "$(dirname "$WSP")" && pwd -P)/$(basename "$WSP")" ;;
esac
[ -x "$WSP" ] || { echo "not executable: $WSP" >&2; exit 2; }
GIT="$(xcrun --find git)"
[ -x "$GIT" ] || { echo "git is required for the macOS confinement gate" >&2; exit 1; }
command -v sysadminctl >/dev/null 2>&1 || {
    echo "sysadminctl is required for the macOS confinement gate" >&2
    exit 1
}
sudo -n true || {
    echo "passwordless sudo is required for the macOS confinement gate" >&2
    exit 1
}

root="$(mktemp -d /private/tmp/wsp-posix.XXXXXX)"
root="$(cd "$root" && pwd -P)"
workspace="$root/workspace"
global="$root/global"
sibling="$root/sibling"
outside="$root/unlisted"
bin="$root/bin"
child="$workspace/child.sh"
user="wspci_$(uuidgen | tr -d - | cut -c1-16)"
pass="$(uuidgen)"
created_user=0

cleanup() {
    if [ "$created_user" -eq 1 ]; then
        sudo sysadminctl -deleteUser "$user" >/dev/null 2>&1 || true
    fi
    sudo rm -rf "$root"
}
trap cleanup EXIT

sudo sysadminctl -addUser "$user" -fullName "$user" -password "$pass" >/dev/null
created_user=1
id "$user" >/dev/null

mkdir -p "$workspace" "$global" "$sibling" "$outside" "$bin"
cp "$WSP" "$bin/wsp"
chmod 755 "$bin/wsp"
printf 'global sentinel\n' > "$global/sentinel"
printf 'sibling sentinel\n' > "$sibling/sentinel"
printf 'outside sentinel\n' > "$outside/sentinel"
"$GIT" init --quiet --initial-branch=main "$workspace/alpha"
"$GIT" -C "$workspace/alpha" config user.email test@example.invalid
"$GIT" -C "$workspace/alpha" config user.name Test
printf 'fixture\n' > "$workspace/alpha/README.md"
"$GIT" -C "$workspace/alpha" add README.md
"$GIT" -C "$workspace/alpha" -c commit.gpgsign=false commit --quiet -m fixture
"$GIT" -C "$workspace/alpha" remote add origin git@test.local:u/alpha.git
printf 'dirty\n' >> "$workspace/alpha/README.md"
cat > "$workspace/.wsp.yaml" <<'YAML'
name: mounted
branch: main
repos:
  test.local/u/alpha:
created: 2026-09-18T00:00:00Z
YAML

cat > "$child" <<CHILD
#!/bin/sh
set -eu
export XDG_DATA_HOME="$global"
export HOME="$global/home"
export PATH="$(dirname "$GIT"):/usr/bin:/bin"
if IFS= read -r ignored < "$global/sentinel"; then exit 10; fi
if ( : > "$global/must-not-create" ); then exit 11; fi
if IFS= read -r ignored < "$sibling/sentinel"; then exit 12; fi
if ( : > "$sibling/must-not-create" ); then exit 13; fi
if IFS= read -r ignored < "$outside/sentinel"; then exit 14; fi
if ( : > "$outside/must-not-create" ); then exit 15; fi
cd "$workspace"
run_wsp() {
    output="\$1"
    shift
    if ! "$bin/wsp" --json "\$@" > "\$output"; then
        cat "\$output" >&2
        exit 20
    fi
}
run_wsp result.json describe "POSIX confined workspace"
run_wsp status.json st
run_wsp repos.json repo ls
grep -q 'description: POSIX confined workspace' .wsp.yaml || { cat .wsp.yaml >&2; exit 21; }
grep -q '"branch": "main"' status.json || { cat status.json >&2; exit 22; }
grep -q '"changed": 1' status.json || { cat status.json >&2; exit 23; }
CHILD
chmod 700 "$child"

# The runner retains the non-workspace paths. The child must traverse the
# fixture root but may only use the two child-owned trees below it.
sudo chown -R "$user":staff "$workspace" "$bin"
chmod 0711 "$root"
chmod 0700 "$global" "$sibling" "$outside"

require_denied() {
    local path="$1"
    if sudo -H -u "$user" /bin/sh -c 'test ! -r "$1" && test ! -w "$1"' /bin/sh "$path"; then
        return
    fi
    echo "confined principal can access protected path: $path" >&2
    exit 1
}
require_write_denied() {
    local path="$1"
    if sudo -H -u "$user" /bin/sh -c 'if ( : > "$1" ) 2>/dev/null; then exit 1; fi' /bin/sh "$path"; then
        return
    fi
    echo "confined principal can create protected path: $path" >&2
    exit 1
}
for protected in "$global/sentinel" "$sibling/sentinel" "$outside/sentinel"; do
    require_denied "$protected"
done
for protected in "$global/must-not-create" "$sibling/must-not-create" "$outside/must-not-create"; do
    require_write_denied "$protected"
done

sudo -H -u "$user" env HOME="$global/home" XDG_DATA_HOME="$global" \
    PATH="$(dirname "$GIT"):/usr/bin:/bin" /bin/sh "$child"

[ "$(cat "$global/sentinel")" = "global sentinel" ]
[ "$(cat "$sibling/sentinel")" = "sibling sentinel" ]
[ "$(cat "$outside/sentinel")" = "outside sentinel" ]
[ ! -e "$global/must-not-create" ]
[ ! -e "$sibling/must-not-create" ]
[ ! -e "$outside/must-not-create" ]
echo "macOS POSIX distinct-principal confinement passed"
