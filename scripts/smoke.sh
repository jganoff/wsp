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
#
# gpgsign is off because the network section commits: a signing key the runner
# does not have would fail the commit rather than the check under test.
cat > "$sandbox/gitconfig" <<'GITCFG'
[user]
	email = smoke@test.local
	name = Smoke Test
[commit]
	gpgsign = false
GITCFG
export GIT_CONFIG_GLOBAL="$sandbox/gitconfig"
# GIT_CONFIG_GLOBAL does not shadow /etc/gitconfig, so a system-level
# core.hooksPath or url.insteadOf would still reach the fixture commits.
export GIT_CONFIG_NOSYSTEM=1

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

# On failure, report what doctor objected to. "doctor reported problems" alone
# means instrumenting this script to find out, and the answer is usually a
# check above having left state behind.
if dout=$("$WSP" doctor 2>&1); then
    ok "doctor (clean)"
else
    bad "doctor in a fresh sandbox: $(printf '%s' "$dout" | grep -v '✓' | tr '\n' '|')"
fi
"$WSP" ls >/dev/null 2>&1 && ok "ls" || bad "ls exited non-zero"

# --size measures disk usage. For a removed workspace the number comes from the
# gc metadata, written when it was removed, so it costs a metadata read rather
# than a walk. Asserted by removing the payload and checking the number holds.
sizews="smoke-du-$$"
"$WSP" new "$sizews" --empty >/dev/null 2>&1
# Anchored on the header row: a workspace whose name contains "size" would
# otherwise satisfy a bare search, which is how the PowerShell twin caught this.
"$WSP" ls --size 2>&1 | grep -qE '^NAME.*SIZE' \
    && ok "ls --size adds a size column" \
    || bad "ls --size printed no SIZE column"
"$WSP" ls 2>&1 | grep -qE '^NAME.*SIZE' \
    && bad "ls without --size printed a SIZE column" \
    || ok "ls without --size leaves the table alone"
"$WSP" rm "$sizews" --force >/dev/null 2>&1
# Read the row for this workspace by name, and empty only this entry: reaching
# across the whole gc directory would target another check's fixture the moment
# this block moves.
reported() { "$WSP" ls --removed --size 2>/dev/null | awk -v n="$sizews" '$1 == n { print $4, $5 }'; }
gcdir=$(find "$XDG_DATA_HOME/wsp/gc" -maxdepth 1 -type d -name "${sizews}__*" | head -1)
before=$(reported)
find "$gcdir" -type f ! -name '.wsp-gc.yaml' -delete 2>/dev/null
after=$(reported)
if [ -n "$before" ] && [ "$before" = "$after" ]; then
    ok "ls --removed --size reads the size recorded at removal"
else
    bad "removed size changed when the files went ($before -> $after), so it was recomputed"
fi
rm -rf "$gcdir"

# Non-interactive setup prints the manual guide instead of prompting, and omits
# the branch-prefix line when one is already configured -- which the check above
# did. Asserting that absence makes this a statement about real config rather
# than "the command ran". `< /dev/null` forces a non-TTY, so it cannot prompt
# and cannot hang a run from a terminal.
out=$("$WSP" setup < /dev/null 2>&1)
if printf '%s' "$out" | grep -qF "requires an interactive terminal" \
    && ! printf '%s' "$out" | grep -qF "config set branch-prefix"; then
    ok "setup declines non-interactively and reflects config"
else
    bad "setup did not print the expected non-interactive guide: $out"
fi

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

# Guides are compiled into the binary, so a build that lost them still passes
# every unit test. Assert on the body, not just the exit code.
"$WSP" help gc 2>&1 | grep -qF "retention-days" \
    && ok "help gc prints the guide" \
    || bad "help gc did not print the gc guide"

# The only non-interactive path through init. The sample is what a user pastes
# into a repo, so it has to contain the key it is a sample of.
"$WSP" init --print-sample 2>&1 | grep -qF "setup_commands" \
    && ok "init --print-sample" \
    || bad "init --print-sample printed no setup_commands key"

# Templates round-trip entirely offline: an unregistered URL is stored verbatim
# and never cloned.
tmpl="smoke-tmpl-$$"
"$WSP" template new "$tmpl" 'git@test.local:user/repo.git' >/dev/null 2>&1
"$WSP" template ls 2>&1 | grep -qF "$tmpl" \
    && ok "template new shows up in template ls" \
    || bad "$tmpl missing from template ls"
# A second template, so removing the first asserts a presence as well as an
# absence. Absence alone is satisfied by a binary that does nothing at all.
"$WSP" template new "$tmpl-keep" 'git@test.local:user/other.git' >/dev/null 2>&1
"$WSP" template rm "$tmpl" >/dev/null 2>&1
remaining=$("$WSP" template ls 2>&1)
if printf '%s' "$remaining" | grep -qF "$tmpl-keep" \
    && ! printf '%s' "$remaining" | grep -qF "$tmpl "; then
    ok "template rm removes one and keeps the other"
else
    bad "template rm left the wrong set: $remaining"
fi
# Leave nothing behind: its repo is not in the registry, so `wsp doctor` in the
# network half would warn and exit non-zero on a template this check created.
"$WSP" template rm "$tmpl-keep" >/dev/null 2>&1

# One local workspace covers the commands that need a workspace but no network.
# Left behind for the sandbox teardown to collect.
lws="smoke-local-$$"
"$WSP" new "$lws" --empty >/dev/null 2>&1

# -- collects the trailing tokens, so a description needs no shell quoting. It
# has to reach the listing, which is the only place a user ever sees it.
"$WSP" describe "$lws" -- described by smoke >/dev/null 2>&1
"$WSP" ls 2>&1 | grep -qF "described by smoke" \
    && ok "describe reaches the ls listing" \
    || bad "the description set by describe is missing from ls"

# Without shell integration cd prints the destination instead of moving. Read
# stdout alone: the "integration not active" hints go to stderr.
out=$("$WSP" cd "$lws" 2>/dev/null)
# Compare against the workspace asked for: "is a workspace" would accept the
# wrong one.
[ "$out" = "$workspaces/$lws" ] \
    && ok "cd prints the workspace path" \
    || bad "cd printed '$out', expected '$workspaces/$lws'"

# rename moves the directory on disk, which is the half that only a real
# filesystem can check — and the half Windows can refuse outright.
"$WSP" rename "$lws" "${lws}-renamed" >/dev/null 2>&1
if [ -d "$workspaces/${lws}-renamed" ] && [ ! -d "$workspaces/$lws" ]; then
    ok "rename moved the workspace directory"
else
    bad "rename did not move $lws to ${lws}-renamed on disk"
fi

if [ -n "$EXPECT" ]; then
    if "$WSP" whatsnew 2>&1 | grep -qF "$EXPECT"; then
        ok "whatsnew shows the expected version"
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

        if dout=$( cd "$ws_dir" && "$WSP" doctor 2>&1 ); then
            ok "doctor in a real workspace"
        else
            bad "doctor in a real workspace: $(printf '%s' "$dout" | grep -v '✓' | tr '\n' '|')"
        fi

        # One unpushed commit is the fixture for the rest of this section: diff
        # bases on the merge-base with upstream, log lists what is unpushed, and
        # rm must refuse to throw it away. Needs a real clone with a real
        # upstream, which is why it lives here and not in the offline half.
        repo_dir="$ws_dir/Hello-World"
        echo smoke > "$repo_dir/smoke.txt"
        if git -C "$repo_dir" add smoke.txt >/dev/null 2>&1 &&
            git -C "$repo_dir" commit --no-verify -m "smoke fixture commit" >/dev/null 2>&1; then
            ok "fixture commit"
        else
            bad "fixture commit failed; the checks below will fail for the wrong reason"
        fi

        "$WSP" diff "$ws" 2>&1 | grep -qF "smoke.txt" \
            && ok "diff shows the change" || bad "diff does not mention smoke.txt"
        "$WSP" log "$ws" 2>&1 | grep -qF "smoke fixture commit" \
            && ok "log shows the unpushed commit" \
            || bad "log does not mention the commit that is ahead of upstream"
        # Proves the command ran inside each clone, not just that exec exited 0.
        # Piped into `grep -q` on purpose. grep closes the pipe as soon as it
        # matches, partway through exec's block-per-repo output, which used to
        # make wsp panic on EPIPE -- so this check covers that fix end to end.
        # It also pins the choice of exit 0 over 141: under `pipefail` a 141
        # here would fail the pipeline even though nothing went wrong.
        "$WSP" exec "$ws" -- git rev-parse --abbrev-ref HEAD 2>&1 | grep -qF "smoke/$ws" \
            && ok "exec runs git in each clone" \
            || bad "exec did not report the workspace branch from the clones"
        # Rewind a branch behind upstream so sync has something to do. A branch
        # that is ahead takes sync's no-op path and reports "already up to date"
        # without moving anything, so asserting that string proves only that the
        # fetch did not fail. Rewind Spoon-Knife, not Hello-World: the latter
        # carries the unpushed commit the rm check below needs.
        git -C "$ws_dir/Spoon-Knife" reset --hard HEAD~1 >/dev/null 2>&1
        out=$("$WSP" sync "$ws" 2>&1)
        # "fast-forwarded", not "rebase": the latter is the ACTION column,
        # printed whether or not anything moved.
        if printf '%s' "$out" | grep -qF "fast-forwarded" &&
            ! printf '%s' "$out" | grep -qF "fetch failed"; then
            ok "sync brings a behind branch up to upstream"
        else
            bad "sync did not move a behind branch: $out"
        fi

        # An unmerged branch must block a plain rm. That guard is the difference
        # between a recoverable mistake and a lost afternoon, and --force is the
        # documented way past it. Assert the reason, not just the exit status:
        # any unrelated failure of rm would satisfy "did not succeed".
        if out=$("$WSP" rm "$ws" --yes 2>&1); then
            bad "rm removed $ws despite unsaved work; --force should be required"
        elif printf '%s' "$out" | grep -qF "unsaved work"; then
            ok "rm refuses a workspace with unsaved work"
        else
            bad "rm failed but not because of unsaved work: $out"
        fi
        "$WSP" rm "$ws" --force >/dev/null 2>&1 \
            && ok "rm --force $ws" || bad "rm --force exited non-zero"
    fi
fi

echo
if [ "$fails" -gt 0 ]; then
    echo "$fails check(s) failed"
    exit 1
fi
echo "all checks passed"
