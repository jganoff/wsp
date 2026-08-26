#!/usr/bin/env bash
# Smoke-test a wsp build end to end. macOS/Linux twin of smoke.ps1.
#
#   ./scripts/smoke.sh --wsp ./wsp --expect-version 0.19.0-rc.1
#   ./scripts/smoke.sh --wsp ./wsp --offline
#
# Runs against a sandboxed data directory, so it never touches your registry,
# mirrors, or workspaces. Exits non-zero if any check fails.
set -uo pipefail

WSP="" EXPECT="" OFFLINE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --wsp) WSP="$2"; shift 2;;
        --expect-version) EXPECT="$2"; shift 2;;
        --offline) OFFLINE=1; shift;;
        *) echo "unknown argument: $1" >&2; exit 2;;
    esac
done
[ -n "$WSP" ] || { echo "usage: $0 --wsp <path> [--expect-version V] [--offline]" >&2; exit 2; }
# Resolve to an absolute path before the cd below moves us: a relative --wsp
# would otherwise break every call. A bare name is left alone; PATH lookup
# does not care about the working directory.
case "$WSP" in
    */*) WSP="$(cd "$(dirname "$WSP")" 2>/dev/null && pwd)/$(basename "$WSP")" ;;
esac
command -v "$WSP" >/dev/null 2>&1 || [ -x "$WSP" ] || { echo "not executable: $WSP" >&2; exit 2; }

fails=0
ok()  { echo "  ok    $1"; }
bad() { echo "  FAIL  $1"; fails=$((fails + 1)); }

# XDG_DATA_HOME isolates config, mirrors, gc and templates on every platform.
# workspaces-dir lives outside that root, so it is set separately below.
sandbox=$(mktemp -d "${TMPDIR:-/tmp}/wsp-smoke.XXXXXX")
export XDG_DATA_HOME="$sandbox/data"
workspaces="$sandbox/workspaces"
mkdir -p "$workspaces"
cleanup() { cd /; rm -rf "$sandbox"; }
trap cleanup EXIT

# Point git at a minimal config so user-level url.insteadOf rewrites (e.g.
# https://github.com/ -> git@github.com:) do not redirect the test clones to
# SSH, where the agent may not be available. Exported into this process only.
cat > "$sandbox/gitconfig" <<'GITCFG'
[user]
	email = smoke@test.local
	name = Smoke Test
GITCFG
export GIT_CONFIG_GLOBAL="$sandbox/gitconfig"

# Commands detect the workspace from the working directory, which
# XDG_DATA_HOME does not isolate. Running from inside a real workspace makes
# doctor and friends inspect *that* one. Move somewhere neutral.
cd "$sandbox" || exit 1

echo "sandbox: $sandbox"
echo
echo "offline"

if v=$("$WSP" --version 2>&1); then
    if [ -n "$EXPECT" ] && ! printf '%s' "$v" | grep -qF "$EXPECT"; then
        bad "--version says '$v', expected to contain '$EXPECT'"
    else
        ok "--version: $v"
    fi
else
    bad "--version exited non-zero: $v"
fi

"$WSP" --help >/dev/null 2>&1 && ok "--help" || bad "--help exited non-zero"

# Shell integration must emit a usable wrapper *and* parse — a grep alone
# would accept syntactically broken output.
for sh in bash zsh; do
    if ! command -v "$sh" >/dev/null 2>&1; then
        echo "  skip  completion $sh (not installed)"
        continue
    fi
    out=$("$WSP" completion "$sh" 2>&1) || { bad "completion $sh exited non-zero"; continue; }
    printf '%s' "$out" | grep -q "wsp" || { bad "completion $sh output has no 'wsp'"; continue; }
    if printf '%s' "$out" | "$sh" -n 2>/dev/null; then
        ok "completion $sh (parses)"
    else
        bad "completion $sh output does not parse as $sh"
    fi
done

# --global is required: both are global-only keys. branch-prefix is set so
# doctor has nothing left to warn about, letting the check below assert a
# clean bill of health rather than merely "it ran".
"$WSP" config set workspaces-dir "$workspaces" --global >/dev/null 2>&1 \
    && ok "config set workspaces-dir" || bad "config set workspaces-dir exited non-zero"
"$WSP" config set branch-prefix smoke --global >/dev/null 2>&1 \
    || bad "config set branch-prefix exited non-zero"

"$WSP" doctor >/dev/null 2>&1 && ok "doctor (clean)" || bad "doctor reported problems in a fresh sandbox"
"$WSP" ls >/dev/null 2>&1 && ok "ls" || bad "ls exited non-zero"

# Removal and recovery, end to end. Worth smoking rather than trusting to unit
# tests: this is the one path where a bug loses a user's work, and an --empty
# workspace exercises all of it without network.
gcws="smoke-gc-$$"
"$WSP" new "$gcws" --empty >/dev/null 2>&1 \
    && ok "new --empty" || bad "new --empty exited non-zero"
"$WSP" rm "$gcws" --force >/dev/null 2>&1 \
    && ok "rm --force" || bad "rm --force exited non-zero"
"$WSP" ls --removed 2>&1 | grep -qF "$gcws" \
    && ok "ls --removed shows the removed workspace" \
    || bad "ls --removed does not list $gcws"
# The footer must survive the empty listing: removing your only workspace is
# exactly when you need to hear that it is recoverable.
"$WSP" ls 2>&1 | grep -qF "recoverable" \
    && ok "ls footer points at the removed workspace" \
    || bad "ls does not mention that something is recoverable"
# Bare `wsp` runs the same listing through a different path, which used to
# overwrite the footer with navigation advice.
"$WSP" 2>&1 | grep -qF "recoverable" \
    && ok "bare wsp keeps the recoverable footer" \
    || bad "bare wsp dropped the recoverable footer"
# Bare `recover` must refuse rather than list: an argumentless read-only form
# is what made the command's name mean two things. Non-zero exit is the contract.
if "$WSP" recover >/dev/null 2>&1; then
    bad "bare recover succeeded; it must ask for a workspace name"
else
    ok "bare recover refuses without a name"
fi
"$WSP" recover "$gcws" >/dev/null 2>&1 \
    && ok "recover <name>" || bad "recover exited non-zero"
"$WSP" ls 2>&1 | grep -qF "$gcws" \
    && ok "recovered workspace is back in ls" \
    || bad "$gcws missing from ls after recover"
"$WSP" rm "$gcws" --force >/dev/null 2>&1

if [ -n "$EXPECT" ]; then
    if "$WSP" whatsnew 2>&1 | grep -qF "$EXPECT"; then
        ok "whatsnew shows $EXPECT"
    else
        bad "whatsnew does not mention $EXPECT (are the release notes in the build?)"
    fi
fi

# wsp cannot register a local path — the registry needs a host/user/repo
# identity and clones over the network — so these steps require connectivity.
if [ "$OFFLINE" -eq 1 ]; then
    echo
    echo "network: skipped (--offline)"
else
    echo
    echo "network"

    "$WSP" registry add https://github.com/octocat/Hello-World.git >/dev/null 2>&1 \
        && ok "registry add" || bad "registry add exited non-zero"

    ws="smoke-$$"
    "$WSP" new "$ws" github.com/octocat/Hello-World >/dev/null 2>&1 \
        && ok "new $ws" || bad "new exited non-zero"

    ws_dir="$workspaces/$ws"
    [ -d "$ws_dir/Hello-World" ] && ok "repo cloned into the workspace" \
        || bad "clone directory missing in $ws_dir"

    if [ -d "$ws_dir" ]; then
        (
            cd "$ws_dir" || exit 1
            "$WSP" st >/dev/null 2>&1 || exit 2
            exit 0
        ) && ok "st" || bad "st exited non-zero"

        # Exercises the fetch-before-clone path on add.
        "$WSP" registry add https://github.com/octocat/Spoon-Knife.git >/dev/null 2>&1
        ( cd "$ws_dir" && "$WSP" repo add github.com/octocat/Spoon-Knife >/dev/null 2>&1 )
        [ -d "$ws_dir/Spoon-Knife" ] && ok "repo add" || bad "repo add did not clone Spoon-Knife"

        ( cd "$ws_dir" && "$WSP" doctor >/dev/null 2>&1 ) \
            && ok "doctor in a real workspace" || bad "doctor (in workspace) exited non-zero"
    fi

    "$WSP" rm "$ws" --yes >/dev/null 2>&1 && ok "rm $ws" || bad "rm exited non-zero"
fi

echo
if [ "$fails" -gt 0 ]; then
    echo "$fails check(s) failed"
    exit 1
fi
echo "all checks passed"
