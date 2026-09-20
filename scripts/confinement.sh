#!/usr/bin/env bash
# Verify macOS-enforced workspace confinement against a real release binary.
#
# The test intentionally uses Seatbelt rather than chmod: the child keeps the
# runner identity, so filesystem mode bits alone would not establish a boundary.
# A missing sandbox-exec is a failure. CI calls this only on macOS.
set -euo pipefail

WSP=""
while [ $# -gt 0 ]; do
    case "$1" in
        --wsp) WSP="$2"; shift 2 ;;
        *) echo "usage: $0 --wsp <path>" >&2; exit 2 ;;
    esac
done

[ "$(uname -s)" = "Darwin" ] || {
    echo "macOS Seatbelt confinement requires Darwin" >&2
    exit 2
}
[ -n "$WSP" ] || { echo "usage: $0 --wsp <path>" >&2; exit 2; }
case "$WSP" in
    */*) WSP="$(cd "$(dirname "$WSP")" && pwd -P)/$(basename "$WSP")" ;;
esac
[ -x "$WSP" ] || { echo "not executable: $WSP" >&2; exit 2; }
GIT="$(xcrun --find git)"
[ -x "$GIT" ] || { echo "git is required for the macOS confinement gate" >&2; exit 1; }
command -v sandbox-exec >/dev/null 2>&1 || {
    echo "sandbox-exec is required for the macOS confinement gate" >&2
    exit 1
}

root="$(mktemp -d "${TMPDIR:-/tmp}/wsp-seatbelt.XXXXXX")"
# Seatbelt compares physical paths. macOS commonly presents temporary paths
# through /var while the kernel evaluates them under /private/var.
root="$(cd "$root" && pwd -P)"
workspace="$root/workspace"
global="$root/global"
sibling="$root/sibling"
outside="$root/unlisted"
data_outside="/System/Volumes/Data$outside"
profile="$root/profile.sb"
child="$workspace/child.sh"
cleanup() { rm -rf "$root"; }
trap cleanup EXIT

mkdir -p "$workspace" "$global" "$sibling" "$outside"
printf 'global sentinel\n' > "$global/sentinel"
printf 'sibling sentinel\n' > "$sibling/sentinel"
printf 'outside sentinel\n' > "$outside/sentinel"
[ -f "$data_outside/sentinel" ] || {
    echo "could not resolve fixture through the macOS Data-volume alias" >&2
    exit 1
}
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

# The profile begins deny-by-default. It exposes the fixture workspace, the
# binary, `/bin/sh`, and the macOS dynamic runtime only. No user home, `/var`,
# `/etc`, or broad executable directory is visible to the child.
cat > "$profile" <<PROFILE
(version 1)
(deny default)
(allow process-fork)
(allow process-exec (literal "/bin/sh"))
(allow process-exec (literal "$WSP"))
(allow process-exec (literal "$GIT"))
(allow file-read* (subpath "/System/Library"))
(allow file-read* (subpath "/usr/lib"))
(allow file-read* (literal "/bin/sh"))
(allow file-read* (literal "/dev/null"))
(allow file-read* (literal "$WSP"))
(allow file-read* (literal "$GIT"))
(allow file-read* (subpath "$workspace"))
(allow sysctl-read)
(allow file-write* (subpath "$workspace"))
(deny file-read* (subpath "$global"))
(deny file-write* (subpath "$global"))
(deny file-read* (subpath "$sibling"))
(deny file-write* (subpath "$sibling"))
PROFILE

cat > "$child" <<CHILD
#!/bin/sh
set -eu
export XDG_DATA_HOME="$global"
export HOME="$global/home"
export PATH="$(dirname "$GIT")"
if IFS= read -r ignored < "$global/sentinel"; then exit 10; fi
if ( : > "$global/must-not-create" ); then exit 11; fi
if IFS= read -r ignored < "$sibling/sentinel"; then exit 12; fi
if ( : > "$sibling/must-not-create" ); then exit 13; fi
if IFS= read -r ignored < "$outside/sentinel"; then exit 14; fi
if ( : > "$outside/must-not-create" ); then exit 15; fi
if IFS= read -r ignored < "$data_outside/sentinel"; then exit 16; fi
if ( : > "$data_outside/must-not-create" ); then exit 17; fi
cd "$workspace"
"$WSP" --json describe "seatbelt confined workspace" > result.json
"$WSP" --json st > status.json
"$WSP" --json repo ls > repos.json
found=0
while IFS= read -r line; do
    [ "\$line" = 'description: seatbelt confined workspace' ] && found=1
done < .wsp.yaml
[ "\$found" -eq 1 ]
found=0
status_branch=0
status_changed=0
while IFS= read -r line; do
    case "\$line" in
        *'"error"'*) exit 18 ;;
        *'"branch": "main"'*) status_branch=1 ;;
        *'"changed": 1'*) status_changed=1 ;;
    esac
done < status.json
[ "\$status_branch" -eq 1 ]
[ "\$status_changed" -eq 1 ]
CHILD
chmod 700 "$child"

sandbox-exec -f "$profile" /bin/sh "$child"

[ "$(cat "$global/sentinel")" = "global sentinel" ]
[ "$(cat "$sibling/sentinel")" = "sibling sentinel" ]
[ "$(cat "$outside/sentinel")" = "outside sentinel" ]
[ ! -e "$global/must-not-create" ]
[ ! -e "$sibling/must-not-create" ]
[ ! -e "$outside/must-not-create" ]
grep -q 'seatbelt confined workspace' "$workspace/result.json"
echo "macOS Seatbelt confinement passed"
