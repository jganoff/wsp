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
pager_out="$sandbox/pager-output"
if PAGER="cat > '$pager_out'" "$WSP" --paginate whatsnew >/dev/null 2>&1 \
    && grep -qF "What's new in wsp" "$pager_out"; then
    ok "pager controls"
else
    bad "--paginate did not route whatsnew through PAGER"
fi
if no_pager_out=$(PAGER="exit 23" "$WSP" --no-pager whatsnew 2>&1) \
    && printf '%s' "$no_pager_out" | grep -qF "What's new in wsp"; then
    ok "no-pager override"
else
    bad "--no-pager invoked PAGER or lost direct output"
fi
if out=$("$WSP" sync --help 2>&1) && printf '%s' "$out" | grep -qF -- "--yes"; then
    ok "sync --abort offers --yes"
else
    bad "sync --help does not expose --yes: $out"
fi
if out=$("$WSP" sync --help 2>&1) && ! printf '%s' "$out" | grep -qF -- "--no-discover"; then
    ok "sync does not offer template discovery"
else
    bad "sync --help exposes --no-discover: $out"
fi

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
if "$WSP" config set workspaces-dir "$workspaces" --global >/dev/null 2>&1 \
    && configured_workspaces=$("$WSP" config get workspaces-dir 2>/dev/null) \
    && [ "$configured_workspaces" = "$workspaces" ]; then
    ok "config set workspaces-dir"
else
    bad "config set workspaces-dir did not isolate the smoke workspaces"
    exit 1
fi
"$WSP" config set branch-prefix smoke --global >/dev/null 2>&1 \
    || bad "config set branch-prefix exited non-zero"

# Exercise persisted policy and invocation overrides without any network work.
for mode in native parallel; do
    if "$WSP" config set progress.mode "$mode" --global >/dev/null 2>&1 \
        && out=$("$WSP" config get progress.mode 2>&1) && [ "$out" = "$mode" ]; then
        ok "git progress mode round-trip $mode"
    else
        bad "git progress mode round-trip $mode: $out"
    fi
    if out=$("$WSP" --git-progress "$mode" config get progress.mode 2>&1) \
        && [ "$out" = "$mode" ]; then
        ok "git progress flag accepts $mode"
    else
        bad "git progress flag $mode: $out"
    fi
done
cp "$XDG_DATA_HOME/wsp/config.yaml" "$sandbox/progress-config-before"
if out=$("$WSP" config set progress.mode automatic --global 2>&1); then
    bad "git progress accepted an invalid config mode"
elif printf '%s' "$out" | grep -qF "progress mode must be 'parallel' or 'native'" \
    && cmp -s "$XDG_DATA_HOME/wsp/config.yaml" "$sandbox/progress-config-before"; then
    ok "git progress rejects invalid config without mutation"
else
    bad "git progress invalid config failed incorrectly or mutated config: $out"
fi
if out=$("$WSP" --git-progress automatic config set progress.mode native --global 2>&1); then
    bad "git progress accepted an invalid flag"
elif printf '%s' "$out" | grep -qF "invalid value" \
    && printf '%s' "$out" | grep -qF -- "--git-progress" \
    && printf '%s' "$out" | grep -qF "parallel" \
    && printf '%s' "$out" | grep -qF "native" \
    && cmp -s "$XDG_DATA_HOME/wsp/config.yaml" "$sandbox/progress-config-before"; then
    ok "git progress rejects invalid flag before work"
else
    bad "git progress invalid flag failed incorrectly or mutated config: $out"
fi
if "$WSP" config set progress.mode native --global >/dev/null 2>&1 \
    && "$WSP" config unset progress.mode --global >/dev/null 2>&1 \
    && out=$("$WSP" config get progress.mode 2>&1) && [ "$out" = "parallel" ]; then
    ok "git progress unset restores default"
else
    bad "git progress unset did not restore parallel: $out"
fi

# On failure, report what doctor objected to. "doctor reported problems" alone
# means instrumenting this script to find out, and the answer is usually a
# check above having left state behind.
if dout=$("$WSP" doctor 2>&1); then
    ok "doctor (clean)"
else
    bad "doctor in a fresh sandbox: $(printf '%s' "$dout" | grep -v '✓' | tr '\n' '|')"
fi
"$WSP" ls >/dev/null 2>&1 && ok "ls" || bad "ls exited non-zero"

# Exercise guidance generation, repair, and removal through the shipped binary.
agentws="smoke-agent-import-$$"
agentdir="$workspaces/$agentws"
if out=$("$WSP" new "$agentws" --empty 2>&1) \
    && [ -f "$agentdir/AGENTS.md" ] \
    && grep -qF '<!-- wsp:begin -->' "$agentdir/AGENTS.md" \
    && [ -f "$agentdir/CLAUDE.md" ] && [ ! -L "$agentdir/CLAUDE.md" ] \
    && printf '@AGENTS.md\n' | cmp -s - "$agentdir/CLAUDE.md"; then
    ok "new generates CLAUDE.md as a regular AGENTS.md import"
    cp "$agentdir/AGENTS.md" "$sandbox/agents-before"
    rm "$agentdir/CLAUDE.md"
    if out=$( cd "$agentdir" && "$WSP" doctor --fix 2>&1 ) \
        && [ -f "$agentdir/CLAUDE.md" ] && [ ! -L "$agentdir/CLAUDE.md" ] \
        && printf '@AGENTS.md\n' | cmp -s - "$agentdir/CLAUDE.md" \
        && cmp -s "$sandbox/agents-before" "$agentdir/AGENTS.md"; then
        ok "doctor repairs CLAUDE.md import without changing AGENTS.md"
    else
        bad "doctor did not restore the regular import or changed AGENTS.md: $out"
    fi
    if out=$("$WSP" rm "$agentws" --json </dev/null 2>&1) \
        && printf '%s' "$out" | grep -qF '"ok": true' \
        && [ ! -d "$agentdir" ]; then
        ok "rm accepts generated CLAUDE.md import without force"
    else
        bad "rm refused a workspace with generated imports: $out"
    fi
else
    bad "new did not generate AGENTS.md and a regular CLAUDE.md import: $out"
fi
# Keep later checks independent if guidance generation or repair failed.
if [ -d "$agentdir" ]; then
    "$WSP" rm "$agentws" --force >/dev/null 2>&1
fi

# Quiet mode is for command substitution, so it must contain precisely the
# workspace names: no table header, metadata, or recoverable-workspace footer.
quietws="smoke-quiet-$$"
"$WSP" new "$quietws" --empty >/dev/null 2>&1
if out=$("$WSP" ls --quiet 2>/dev/null) && [ "$out" = "$quietws" ]; then
    ok "ls --quiet prints workspace names"
else
    bad "ls --quiet printed '$out', expected '$quietws'"
fi
"$WSP" rm "$quietws" --force >/dev/null 2>&1

# Quiet output becomes positional arguments in a caller, so no malformed
# workspace directory may emit a flag that changes the removal of its victim.
victim="smoke-quiet-victim-$$"
malformed="$workspaces/--force"
quietout="$sandbox/quiet-invalid.stdout"
quieterr="$sandbox/quiet-invalid.stderr"
"$WSP" new "$victim" --empty >/dev/null 2>&1
touch "$workspaces/$victim/user-file"
mkdir "$malformed"
cp "$workspaces/$victim/.wsp.yaml" "$malformed/.wsp.yaml"
if "$WSP" ls -q >"$quietout" 2>"$quieterr"; then
    bad "ls --quiet accepted an invalid workspace name"
elif [ -s "$quietout" ]; then
    bad "ls --quiet emitted names before rejecting an invalid workspace name"
elif "$WSP" rm $("$WSP" ls -q 2>/dev/null) --yes >/dev/null 2>&1; then
    bad "invalid ls --quiet output allowed a forced victim removal"
elif [ -d "$workspaces/$victim" ]; then
    ok "ls --quiet rejects invalid workspace names"
else
    bad "invalid ls --quiet output removed its protected victim"
fi
rm -f "$malformed/.wsp.yaml"
rmdir "$malformed"
"$WSP" rm "$victim" --force >/dev/null 2>&1

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

# Explicit workspace names are processed in order in one invocation.
batchone="smoke-batch-one-$$"
batchtwo="smoke-batch-two-$$"
"$WSP" new "$batchone" --empty >/dev/null 2>&1
"$WSP" new "$batchtwo" --empty >/dev/null 2>&1
if out=$("$WSP" rm --force --json -- "$batchone" "$batchtwo" 2>/dev/null) \
    && printf '%s\n' "$out" | grep -qF '"removals"' \
    && [ "$(printf '%s\n' "$out" | grep -Fc '"ok": true')" -eq 2 ] \
    && [ -z "$("$WSP" ls -q 2>/dev/null)" ]; then
    ok "rm removes multiple explicitly named workspaces"
else
    bad "rm multiple workspace names failed"
fi

# Successful text batches show one deadline per workspace and shared
# recovery instructions once for the whole batch.
textbatchone="smoke-rm-text-batch-one-$$"
textbatchtwo="smoke-rm-text-batch-two-$$"
textbatchthree="smoke-rm-text-batch-three-$$"
textbatcherr="$sandbox/rm-text-batch.stderr"
if "$WSP" new "$textbatchone" --empty >/dev/null 2>&1 \
    && "$WSP" new "$textbatchtwo" --empty >/dev/null 2>&1 \
    && "$WSP" new "$textbatchthree" --empty >/dev/null 2>&1; then
    if textbatchout=$("$WSP" rm --force -- "$textbatchone" "$textbatchtwo" "$textbatchthree" 2>"$textbatcherr") \
    && [ "$(printf '%s\n' "$textbatchout" | grep -Fc 'wsp recover ')" -eq 1 ] \
    && [ "$(printf '%s\n' "$textbatchout" | grep -Fc 'recoverable until ')" -eq 3 ] \
    && [ "$(printf '%s\n' "$textbatchout" | grep -Fc 'lists all recoverable workspaces.')" -eq 1 ] \
    && printf '%s\n' "$textbatchout" | grep -qF 'Restore one with `wsp recover <name>`; `wsp ls --removed` lists all recoverable workspaces.' \
    && printf '%s\n' "$textbatchout" | grep -qF "Workspace \"$textbatchone\" removed, recoverable until " \
    && printf '%s\n' "$textbatchout" | grep -qF "Workspace \"$textbatchtwo\" removed, recoverable until " \
    && printf '%s\n' "$textbatchout" | grep -qF "Workspace \"$textbatchthree\" removed, recoverable until " \
    && ! /usr/bin/grep -q "$(printf '\033')" "$textbatcherr"; then
        ok "rm text batch shows deadlines per workspace and recovery guidance once"
    else
        bad "rm text batch output was wrong: $textbatchout"
    fi
else
    bad "rm text batch fixture creation failed; did not force-remove workspaces"
fi

# If an earlier batch item succeeds and the next needs confirmation, the
# diagnostic must name that next item. A non-TTY caller must stop there.
confirmfirst="smoke-rm-confirm-first-$$"
confirmpartial="smoke-rm-confirm-partial-$$"
confirmlater="smoke-rm-confirm-later-$$"
"$WSP" new "$confirmfirst" --empty >/dev/null 2>&1
mkdir "$workspaces/$confirmpartial"
"$WSP" new "$confirmlater" --empty >/dev/null 2>&1
confirmout="$sandbox/rm-confirm.stdout"
confirmerr="$sandbox/rm-confirm.stderr"
if "$WSP" rm "$confirmfirst" "$confirmpartial" "$confirmlater" </dev/null >"$confirmout" 2>"$confirmerr"; then
    bad "rm batch unexpectedly confirmed a partial workspace without a TTY"
elif grep -qF "pass --yes to confirm: wsp rm \"$confirmpartial\" --yes" "$confirmerr" \
    && [ ! -d "$workspaces/$confirmfirst" ] \
    && [ -d "$workspaces/$confirmpartial" ] \
    && [ -d "$workspaces/$confirmlater" ]; then
    ok "rm batch non-TTY error names current workspace"
else
    bad "rm batch non-TTY error or stop point was wrong: $(tr '\n' '|' <"$confirmerr")"
fi
"$WSP" rm "$confirmpartial" --yes >/dev/null 2>&1
"$WSP" rm "$confirmlater" --force >/dev/null 2>&1

# Give the batch a real terminal so the prompt itself is checked, not only the
# non-TTY error. `script` has different command syntax on macOS and Linux.
promptfirst="smoke-rm-prompt-first-$$"
promptpartial="smoke-rm-prompt-partial-$$"
promptlater="smoke-rm-prompt-later-$$"
"$WSP" new "$promptfirst" --empty >/dev/null 2>&1
mkdir "$workspaces/$promptpartial"
"$WSP" new "$promptlater" --empty >/dev/null 2>&1
if ! command -v script >/dev/null 2>&1; then
    bad "rm interactive prompt test needs script"
elif [ "$(uname -s)" = Darwin ]; then
    promptoutput=$(printf 'n\n' | WSP="$WSP" PROMPT_FIRST="$promptfirst" PROMPT_PARTIAL="$promptpartial" PROMPT_LATER="$promptlater" \
        script -q /dev/null /bin/sh -c 'exec "$WSP" rm "$PROMPT_FIRST" "$PROMPT_PARTIAL" "$PROMPT_LATER"' 2>&1)
else
    promptoutput=$(printf 'n\n' | WSP="$WSP" PROMPT_FIRST="$promptfirst" PROMPT_PARTIAL="$promptpartial" PROMPT_LATER="$promptlater" \
        script -q -c 'exec "$WSP" rm "$PROMPT_FIRST" "$PROMPT_PARTIAL" "$PROMPT_LATER"' /dev/null 2>&1)
fi
if [ -n "${promptoutput:-}" ] \
    && printf '%s' "$promptoutput" | grep -qF "Remove workspace \"$promptpartial\"? [y/N]:" \
    && ! printf '%s' "$promptoutput" | grep -qF "Remove workspace \"$promptfirst\"? [y/N]:" \
    && [ ! -d "$workspaces/$promptfirst" ] \
    && [ -d "$workspaces/$promptpartial" ] \
    && [ -d "$workspaces/$promptlater" ]; then
    ok "rm interactive prompt names current workspace"
else
    bad "rm interactive prompt or decline was wrong: ${promptoutput:-<no output>}"
fi
"$WSP" rm "$promptpartial" --yes >/dev/null 2>&1
"$WSP" rm "$promptlater" --force >/dev/null 2>&1

# A batch reports completed work and its first failure. The final workspace
# must be untouched so users can fix the error and rerun it explicitly.
failfirst="smoke-rm-first-$$"
faillater="smoke-rm-later-$$"
failmissing="smoke-rm-missing-$$"
jsonerr="$sandbox/rm-batch-json.stderr"
"$WSP" new "$failfirst" --empty >/dev/null 2>&1
"$WSP" new "$faillater" --empty >/dev/null 2>&1
if out=$("$WSP" rm "$failfirst" "$failmissing" "$faillater" --yes --json 2>"$jsonerr"); then
    bad "rm batch unexpectedly succeeded after a missing workspace"
else
    remaining=$("$WSP" ls -q 2>/dev/null)
    if printf '%s\n' "$out" | grep -qF '"removals"' \
        && printf '%s\n' "$out" | grep -qF "\"workspace\": \"$failfirst\"" \
        && printf '%s\n' "$out" | grep -qF "\"workspace\": \"$failmissing\"" \
        && printf '%s\n' "$out" | grep -qF '"ok": false' \
        && ! grep -qF "Failed to remove workspace \"$failmissing\"" "$jsonerr" \
        && printf '%s\n' "$remaining" | grep -Fx "$faillater" >/dev/null \
        && ! printf '%s\n' "$remaining" | grep -Fx "$failfirst" >/dev/null; then
        ok "rm reports and stops at first batch failure"
    else
        bad "rm batch failure output or stopping point was wrong: $out"
    fi
fi

# The failed batch intentionally leaves its later workspace behind; clean it
# before the checks that assume an empty active listing.
"$WSP" rm "$faillater" --force >/dev/null 2>&1

# In text mode, completed removals remain pipeable while the failed removal is
# diagnostic output. The process still reports failure after rendering both.
textfirst="smoke-rm-text-first-$$"
textlater="smoke-rm-text-later-$$"
textmissing="smoke-rm-text-missing-$$"
texterr="$sandbox/rm-batch.stderr"
"$WSP" new "$textfirst" --empty >/dev/null 2>&1
"$WSP" new "$textlater" --empty >/dev/null 2>&1
if textout=$("$WSP" rm "$textfirst" "$textmissing" "$textlater" --yes 2>"$texterr"); then
    bad "rm text batch unexpectedly succeeded after a missing workspace"
elif printf '%s\n' "$textout" | grep -qF "Workspace \"$textfirst\" removed, recoverable until " \
    && ! printf '%s\n' "$textout" | grep -qF "Failed to remove workspace \"$textmissing\"" \
    && grep -qF "Failed to remove workspace \"$textmissing\"" "$texterr" \
    && printf '%s\n' "$("$WSP" ls -q 2>/dev/null)" | grep -Fx "$textlater" >/dev/null; then
    ok "rm text sends batch failures to stderr"
else
    bad "rm text batch did not separate output streams"
fi
"$WSP" rm "$textlater" --force >/dev/null 2>&1

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

# Access checks must opt into remote work, observe real Git success/failure,
# leave settings untouched, and keep JSON separate from progress output.
access_source="$sandbox/access-source"
access_url="https://github.com/smoke/access"
access_trace="$sandbox/access-trace"
access_json="$sandbox/access.json"
if git init -q --initial-branch=main "$access_source" \
    && git -C "$access_source" commit -q --allow-empty -m initial \
    && git config --file "$sandbox/gitconfig" "url.file://$access_source.insteadOf" "$access_url" \
    && "$WSP" registry add "$access_url" >/dev/null 2>&1; then
    cp "$XDG_DATA_HOME/wsp/config.yaml" "$sandbox/access-config-before"
    : > "$access_trace"
    GIT_TRACE2_EVENT="$access_trace" "$WSP" doctor --json > "$access_json" 2> "$sandbox/access-stderr"
    if jq -e '[.checks[] | select(.check == "git-access")] | length == 0' "$access_json" >/dev/null \
        && ! rg -q '"ls-remote"' "$access_trace"; then
        ok "doctor keeps remote access opt-in"
    else
        bad "plain doctor checked remote access"
    fi
    for accessCommand in setup doctor; do
        : > "$access_trace"
        if GIT_TRACE2_EVENT="$access_trace" "$WSP" "$accessCommand" --check-access --git-progress native --json \
            < /dev/null > "$access_json" 2> "$sandbox/access-stderr" \
            && jq -e '[.checks[] | select(.check == "git-access")] | length == 1 and all(.[]; .details.result == "succeeded" and .details.mode == "parallel")' "$access_json" >/dev/null \
            && rg -q '"ls-remote"' "$access_trace" \
            && cmp -s "$XDG_DATA_HOME/wsp/config.yaml" "$sandbox/access-config-before"; then
            ok "$accessCommand --check-access observes real access without changing config"
        else
            bad "$accessCommand --check-access failed or returned invalid JSON"
        fi
    done
    mv "$access_source" "$sandbox/access-source-offline"
    : > "$access_trace"
    if GIT_TRACE2_EVENT="$access_trace" "$WSP" setup --check-access --json \
        < /dev/null > "$access_json" 2> "$sandbox/access-stderr"; then
        bad "access check succeeded after its remote disappeared"
    elif jq -e '[.checks[] | select(.check == "git-access")] | length == 1 and all(.[]; .details.result == "failed" and .details.cause == "unknown" and (.message | contains("--git-progress native")))' "$access_json" >/dev/null \
        && [ "$(jq -s '[.[] | select(.event == "start" and (.argv | index("ls-remote")))] | length' "$access_trace")" = 1 ] \
        && cmp -s "$XDG_DATA_HOME/wsp/config.yaml" "$sandbox/access-config-before"; then
        ok "access failure offers native mode without retry or config mutation"
    else
        bad "access failure lost its structured result or retried"
    fi
    "$WSP" registry rm access >/dev/null 2>&1 || bad "access fixture cleanup failed"
else
    bad "access fixture setup failed"
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

# Backdate only the expired fixture, without waiting or changing retention.
purgews="smoke-gc-readonly-$$"
if "$WSP" new "$purgews" --empty >/dev/null 2>&1 \
    && "$WSP" rm "$purgews" --force >/dev/null 2>&1; then
    purge_dirs=("$XDG_DATA_HOME/wsp/gc/${purgews}__"*)
    recent_dirs=("$XDG_DATA_HOME/wsp/gc/${gcws}__"*)
    if [ "${#purge_dirs[@]}" -eq 1 ] && [ -d "${purge_dirs[0]}" ] \
        && [ "${#recent_dirs[@]}" -eq 1 ] && [ -d "${recent_dirs[0]}" ]; then
        expired="${purge_dirs[0]}"
        recent="${recent_dirs[0]}/module"
        outside="$sandbox/external-module"
        mkdir -p "$expired/cache/module" "$recent" "$outside"
        printf 'expired module\n' > "$expired/cache/module/source.go"
        printf 'recent module\n' > "$recent/source.go"
        printf 'external module\n' > "$outside/source.go"
        ln -s "$outside" "$expired/cache/module/external-link"
        ln -s missing "$expired/cache/module/dangling-link"
        awk '/^trashed_at:/ {$0 = "trashed_at: 2000-01-01T00:00:00Z"} {print}' \
            "$expired/.wsp-gc.yaml" > "$sandbox/expired-meta"
        mv "$sandbox/expired-meta" "$expired/.wsp-gc.yaml"
        chmod 555 "$expired/cache/module" "$expired" "$recent" "$outside"
        purge_out=$("$WSP" doctor --fix --json 2>"$sandbox/gc-stderr")
        purge_rc=$?
        if [ "$purge_rc" -eq 0 ] && [ ! -e "$expired" ] \
            && printf '%s' "$purge_out" | jq -e \
                '.checks[] | select(.check == "gc-stale-entries") | .message == "purged 1 stale gc entries"' >/dev/null; then
            ok "gc purges expired read-only directories"
        else
            bad "gc failed to purge read-only fixture: $purge_out $(cat "$sandbox/gc-stderr")"
        fi
        if [ "$(cat "$recent/source.go")" = 'recent module' ] \
            && [ "$(LC_ALL=C ls -ld "$recent" | cut -c1-10)" = 'dr-xr-xr-x' ]; then
            ok "gc preserves recent contents and permissions"
        else
            bad "gc changed the recent module fixture"
        fi
        if [ "$(cat "$outside/source.go")" = 'external module' ] \
            && [ "$(LC_ALL=C ls -ld "$outside" | cut -c1-10)" = 'dr-xr-xr-x' ]; then
            ok "gc preserves external link target contents and permissions"
        else
            bad "gc changed the external link target"
        fi
        # Ensure failed assertions cannot leave unwritable temporary fixtures.
        chmod -R u+w "$expired" "$recent" "$outside" 2>/dev/null || true
    else
        bad "gc fixture paths are ambiguous"
    fi
else
    bad "gc read-only fixture setup failed"
fi

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

# Navigation must not invoke Git, even when a populated mirror has newer refs.
# Trace2 is checked with a positive control, so a missing trace cannot silently
# turn a broken fixture into a passing check.
cdws="smoke-cd-$$"
cd_dir="$workspaces/$cdws"
cd_source="$sandbox/cd-source"
cd_clone="$cd_dir/widgets"
cd_mirror="$XDG_DATA_HOME/wsp/mirrors/github.com/smoke/widgets.git"
cd_trace="$sandbox/cd-git-trace"
if mkdir -p "$cd_dir" "$(dirname "$cd_mirror")" \
    && git init -q --initial-branch=main "$cd_source" \
    && git -C "$cd_source" commit -q --allow-empty -m initial \
    && git clone -q "$cd_source" "$cd_clone" \
    && cd_before=$(git -C "$cd_clone" rev-parse origin/main) \
    && git -C "$cd_source" commit -q --allow-empty -m newer \
    && git clone -q --bare "$cd_source" "$cd_mirror" \
    && git -C "$cd_mirror" update-ref refs/remotes/origin/main HEAD \
    && cd_newer=$(git -C "$cd_mirror" rev-parse refs/remotes/origin/main) \
    && [ "$cd_before" != "$cd_newer" ] \
    && GIT_TRACE2_EVENT="$cd_trace" git -C "$cd_clone" rev-parse HEAD >/dev/null \
    && [ -s "$cd_trace" ]; then
    cat > "$cd_dir/.wsp.yaml" <<YAML
name: $cdws
branch: smoke/$cdws
repos:
  github.com/smoke/widgets: null
created: 2026-01-01T00:00:00Z
YAML
    printf 'navigation canary\n' > "$cd_clone/.git/FETCH_HEAD"
    : > "$cd_trace"
    if out=$(WSP_SHELL=1 GIT_TRACE2_EVENT="$cd_trace" "$WSP" cd "$cdws" 2>"$sandbox/cd.stderr") \
        && [ "$out" = "$cd_dir" ] \
        && [ ! -s "$cd_trace" ] \
        && [ "$(git -C "$cd_clone" rev-parse origin/main)" = "$cd_before" ] \
        && [ "$(cat "$cd_clone/.git/FETCH_HEAD")" = "navigation canary" ]; then
        ok "cd leaves git untouched"
    else
        bad "cd invoked git, changed refs/FETCH_HEAD, or returned the wrong path: $out"
    fi
else
    bad "cd read-only fixture could not establish a newer mirror and working Git trace"
fi
rm -rf "$cd_dir" "$cd_mirror"

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

        # Add directly from a workspace whose HOME and wsp data directory do
        # not exist. This is the portable agent path: Spoon-Knife is deliberately
        # absent from the host registry, so success proves it cloned from origin
        # without creating host mirror or registry state.
        isolated_data="$sandbox/isolated-data"
        if ( cd "$ws_dir" && env -i PATH="$PATH" HOME="$sandbox/isolated-home" USERPROFILE="$sandbox/isolated-home" XDG_DATA_HOME="$isolated_data" GIT_CONFIG_NOSYSTEM=1 "$WSP" repo add https://github.com/octocat/Spoon-Knife.git >/dev/null 2>&1 ) &&
            [ -d "$ws_dir/Spoon-Knife" ] && [ ! -e "$isolated_data" ]; then
            ok "repo add works from an isolated workspace"
        else
            bad "isolated repo add did not clone directly without global state"
        fi
        # Repeating the isolated add on the host is membership-first. It must
        # not turn a durable workspace member into a global registry/mirror
        # entry just because global infrastructure is now reachable.
        spoon_origin=$(git -C "$ws_dir/Spoon-Knife" remote get-url origin 2>/dev/null)
        if ( cd "$ws_dir" && "$WSP" repo add https://github.com/octocat/Spoon-Knife.git 2>&1 ) | grep -qiF "already" &&
            ! "$WSP" registry ls 2>&1 | grep -qF "Spoon-Knife" &&
            [ "$(git -C "$ws_dir/Spoon-Knife" remote get-url origin 2>/dev/null)" = "$spoon_origin" ]; then
            spoon_mirror=$(find "$XDG_DATA_HOME/wsp/mirrors" -type d -name 'Spoon-Knife.git' -print -quit 2>/dev/null)
            [ -z "$spoon_mirror" ] && ok "host retry keeps isolated repo workspace-local" \
                || bad "host retry created a mirror for the isolated repo"
        else
            bad "host retry registered or failed to recognize isolated repo"
        fi

        if dout=$( cd "$ws_dir" && "$WSP" doctor 2>&1 ); then
            ok "doctor accepts an unregistered workspace repo"
        else
            bad "doctor after isolated add: $(printf '%s' "$dout" | grep -v '✓' | tr '\n' '|')"
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
