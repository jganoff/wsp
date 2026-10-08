# Slow-operation progress audit

**Date:** 2026-10-08
**Status:** Source audit complete; proposed implementation slices, not implemented.
**Scope:** All commands registered in `crates/wsp/src/cli/mod.rs`, their nested
commands, shared startup, subprocesses, locks, and expensive filesystem work.
Three parallel audits covered network, mutations, and reads/support commands.

The [implementation plan](2026-10-08-progress-fix-plan.md) turns the suggested
slices below into delivery patches, output policies, and verification gates.

## Contract and evidence limits

The governing [design tenets](../../design-tenets.md) are:

- **Fast is invisible:** local work that finishes quickly leaves no progress.
- **Slow is legible:** say what is being fetched and how much before waiting;
  silence past a second is a bug.
- **Reads stay local:** progress must not introduce network into local reads.
- **Structured output is the contract:** stdout remains the command's JSON
  document in JSON mode; operational feedback belongs on stderr.

The reported new-machine pause is consistent with the source findings below.
This audit did not reproduce that machine's network delay or determine whether
it was DNS, SSH negotiation, authentication, remote enumeration, or disk work.
The buffering and feedback omissions are confirmed code paths; their observed
duration on a particular machine is not measured. Priorities below are proposed
fix order, not measured frequency or existing GitHub issue labels.

A static header is weaker than useful progress: it identifies neither changing
activity nor the remaining repository. Unknown work needs an activity indicator
and elapsed time, not a fabricated percentage. Known transfer/count information
should be retained when Git provides it.

## Shared causes

| ID | Priority | Finding | Source evidence |
| --- | --- | --- | --- |
| S1 | P1 | Renderer reveals after 500 ms, then waits indefinitely for caller updates. Quiet operations leave a frozen line. | `crates/wsp-core/src/progress.rs:13`, `:158-166` |
| S2 | P1 | Staged fetch uses `git fetch --porcelain` without `--progress`, then `.output()` captures all output. A display wrapper cannot stream information it never receives. | `crates/wsp-core/src/git.rs:723-735`, `:783-815` |
| S3 | P1 | Clone parser accepts only completed percentage records. Non-percentage enumeration, count/byte/rate details, diagnostics, and partial lines are discarded from the live display. | `crates/wsp-core/src/git.rs:321-370`, `:383-415` |
| S4 | P1 | Lock acquisition retries silently every 50 ms for up to 30 seconds; holder information appears only on timeout. | `crates/wsp-core/src/filelock.rs:13-14`, `:40-75` |

Piped diagnostics can hide messages needing attention. SSH may use `/dev/tty`,
so this is not proof of a hidden SSH prompt causing the reported pause. The
runner needs an explicit diagnostic/prompt rendering policy, including partial
lines, without treating arbitrary stderr as a trustworthy phase or exposing
credential-bearing URLs in labels.

## Command and phase inventory

Canonical command names below come from the current CLI registration. Fetch is
`wsp repo fetch`; add/remove are `wsp repo add` / `wsp repo rm`. Some helper
comments and historical terminology use top-level spellings.

| ID | Priority | Command / phase | Current feedback and gap | Evidence |
| --- | --- | --- | --- | --- |
| N1 | P1 | `repo fetch`; live `sync` refresh, mirror and direct origin | One `Fetching N repo(s)...` header followed by completion lines. No display updates while workers run, no active identities/phases. | `crates/wsp/src/cli/fetch.rs:406-448`; `cli/sync.rs:257`; `transport.rs:62`, `:104` |
| N2 | P1 | `new` / `repo add` single-mirror prefetch; `repo fetch --all` single repo | Static `Fetching...` around the buffered backend; no transfer events. | `crates/wsp-core/src/git.rs:299-303`; `crates/wsp/src/cli/fetch.rs:39`, `:256` |
| N3 | P1 | Bare mirror and local mirror clones: registry addition, automatic registration, template auto-registration, doctor repair, workspace creation/addition | Delayed `Cloning...`, then percentage bars if Git emits them. Quiet starts and object copying remain static; initial mirror fetch after cloning is separately buffered. | `crates/wsp-core/src/git.rs:205-214`, `:593-619`; `crates/wsp/src/cli/repo.rs:133-138`, `:290-292`; `cli/add.rs:298-300`; `cli/template.rs:657`; `cli/doctor.rs:1763` |
| N4 | P1 | Workspace-local `repo add` direct clone | Captured direct clone has no progress wrapper. Subsequent branch checkout also has no phase feedback. | `crates/wsp-core/src/git.rs:218-224`, `:805`; `workspace_add.rs:265-273`, `:341` |
| N5 | P2 | `repo fetch --all`; multi-mirror `new` / `repo add` prefetch | Aggregate bar advances only when a repo finishes. 0/N or N-1/N can freeze without identifying unfinished work. | `crates/wsp/src/cli/fetch.rs:85-135` |
| N6 | P2 | All staged fetches; `repo fetch --all` propagation | Repacking, import lock, pack indexing, and ref publication lack phase feedback. `--all` finishes its mirror display before propagating into workspaces. | `crates/wsp-core/src/git.rs:735`, `:829-909`; `crates/wsp/src/cli/fetch.rs:291-309` |
| N7 | P2 | `registry add --from ...` | Captured `gh repo list` has no named waiting phase. | `crates/wsp/src/cli/repo.rs:394-408` |
| N8 | P2 | `sync` apply/continue/abort, including dry-run local checks | Rebase/merge/continue/abort and local inspection run synchronously; final results arrive after collection. Prior fetch feedback can remain the last visible activity. | `crates/wsp/src/cli/sync.rs:228`, `:263-270`, `:323`, `:446-447`, `:654-824` |
| M1 | P1 | `rm` safety checks before confirmation | Local safety checks and network fetch occur before the removal announcement. User cannot tell whether the command is checking safety or stalled. | `crates/wsp/src/cli/delete.rs:125`, `:276`; `crates/wsp-core/src/workspace.rs:2315-2358` |
| M2 | P1 | `rm` measuring/moving; `recover` restoring | Removal has a static header; recovery is silent until success. Recursive sizing, clearing a partial destination, cross-filesystem copying, and deletion can take substantial time. | `crates/wsp-core/src/gc.rs:69-127`, `:488`, `:584-621`; `crates/wsp/src/cli/recover.rs:68` |
| M3 | P2 | `repo rm` safety and clone deletion | Aggregate `Removing N repo(s)` header; no changing per-repo check/deletion status during recursive work. | `crates/wsp/src/cli/remove.rs:66`; `crates/wsp-core/src/workspace.rs:1164`, `:1359`, `:1506` |
| M4 | P2 | `new` / `repo add` post-clone processing | `new` names each clone; mirror-based add has generic clone progress. Neither covers subsequent validation, ref propagation, checkout, configuration, and publication with accurate phases. | `crates/wsp-core/src/workspace.rs:524`, `:2741`, `:2766-2778`, `:2858-2894`; `workspace_add.rs:339-352`; `crates/wsp/src/cli/add.rs:133`, `:161` |
| M5 | P2 | `new`, `repo add`, `repo rm` language integration | Go detection and generation recursively scan with no status; detection/application can traverse twice, including non-Go repositories. | `crates/wsp-core/src/lang/mod.rs:47-51`; `lang/go.rs:28-37`, `:112`, `:139` |
| M6 | P2 | Setup commands in `new`, `repo add`, `repo setup` | Repo-level approval/announcement, then blocking child execution. Child output is inherited and already live, but quiet commands and transitions are unlabeled. | `crates/wsp-core/src/setup_runner.rs:65`, `:114`, `:149` |
| M7 | P2 | `setup` GitHub username detection | Captured `gh api user` runs between setup steps without naming the network wait. | `crates/wsp/src/cli/setup.rs:112-115`, `:139` |
| M8 | P3 | `rename` branch changes/rollback; `new` adoption/configuration | Serial Git work without current repo/phase; adoption is announced after validation. | `crates/wsp/src/cli/rename.rs:79`; `crates/wsp-core/src/workspace.rs:516-522`, `:2587-2618`, `:2858-2894` |
| M9 | P2 | Automatic GC on mutating flows | Scan is silent; purge already names workspace and i/N, but one large deletion leaves static feedback. | `crates/wsp-core/src/gc.rs:636-684`, `:724-756` |
| R1 | P2 | `st` local inspection | Multiple captured Git queries; parallel collection waits silently for the slowest repo. | `crates/wsp/src/cli/status.rs:118-196` |
| R2 | P2 | `st` configured PR lookup; `rm` PR checks | Immediate count header then captured parallel `gh` calls; no active identity/completion changes. Configured network is existing behavior, not a proposed addition. | `crates/wsp/src/cli/status.rs:202-219`; `pr.rs:30-44`, `:67-96`; `cli/delete.rs:182` |
| R3 | P2 | `diff`, `log` | Serial captured upstream/history/diff work before rendering. Large histories and configured diff helpers can leave the terminal silent. | `crates/wsp/src/cli/diff.rs:64-95`, `:132-139`; `cli/log.rs:65-116`, `:178` |
| R4 | P2 | `ls --size`; removed-entry size fallback | Full recursive tree measurement before results; ordinary removed entries usually use recorded size. | `crates/wsp/src/cli/list.rs:200-207`, `:262-266`; `crates/wsp-core/src/util.rs:46-54` |
| R5 | P2 | `doctor`, including repair | Existing headings/check results name completed work, not necessarily the active slow check. Recursive trash sizing and captured Git checks can stall between results; repair inherits network/mutation gaps. | `crates/wsp/src/cli/doctor.rs:55`, `:165-216`, `:320-348`, `:1641-1646` |
| R6 | P2 | Template discovery after registration/addition | Local enumeration and captured mirror `ls-tree` / per-template `show`, without a discovery phase. Choice prompts themselves are already visible. | `crates/wsp-core/src/discovery.rs:39`, `:92-102`, `:235-280` |
| R7 | P3 | Common context/startup; pager preparation | Filesystem capability/config/ancestor reads precede dispatch. Pager discovery captures local Git subprocesses and eagerly computes fallbacks. Slow mounts may block these; no observed runtime reproduction. | `crates/wsp/src/main.rs:100`; `context.rs:24-62`, `:114`, `:198`; `pager.rs:98-106`, `:163-233` |

### Remaining command coverage and intentional waits

| Commands / modes | Disposition |
| --- | --- |
| Default invocation, ordinary `ls`, `repo` / `repo ls`, `cd`, `config ls/get`, `registry` / `registry ls`, template list/show/export | Local metadata work; no independent heavy phase found. Share startup/filesystem risk R7. `ls` also reads removed-workspace metadata. |
| `describe`, `config set/unset`, registry mutations, template mutations, `repo setup-commands` mutations | Share lock risk S4; ordinary small metadata operations need no unconditional progress. |
| `registry rm` | Existing mirror-removal header; recursive deletion shares M3/M9 style feedback gaps (`crates/wsp/src/cli/repo.rs:511`). |
| `template new/import/rm/rename`, `template repo add/rm`, `template config set/get/unset`, `template agent-md set/unset`, `template setup-commands ls/add/rm/clear` | Ordinary local metadata work, S4/R7 as applicable. Auto-registration inherits N3; discovery inherits R6. No separate template progress system needed. |
| `repo setup-commands ls/add/rm/clear`, `init`, generated agent guidance/skills | Primarily small local reads/writes, S4/R7 as applicable. `init` local Git detection is a contingent quiet subprocess wait (`cli/init.rs:72`). Its visible input prompt is intentional. |
| `exec` human mode | Prints repo/command and inherits child streams (`cli/exec.rs:92-95`, `:244`). User owns child runtime and interaction. Do not overlay a spinner or impose a timeout on a child TUI. |
| `exec --json` | Intentionally captures child streams and nulls stdin (`cli/exec.rs:215-226`). Optional delayed stderr status is a policy choice; do not change the captured-output contract. |
| Confirmation/approval/discovery prompts; pager viewing | Expected user waits. Stop transient progress before handing over interaction; no heartbeat required while awaiting the user. |
| `completion` generation and dynamic candidates | Local reads, no independent network gap. Slow disk is possible, but shell completion must stay silent: avoid redundant reads and investigate bounded work rather than showing progress in the completion protocol. |
| `help`, `--help`, `whatsnew`, developer `generate` | Embedded content/CLI introspection. No specialized heavy phase found. Pager/context risks apply where those paths are used. |

## Proposed fix plan

### Slice 1: Shared contract and the reported network experience

1. Retain the 500 ms delayed reveal. Add a timed activity/elapsed update so quiet
   work cannot freeze indefinitely. Fast completion cancels all rendering.
2. Extend the existing sanitized Git execution path to retain porcelain stdout
   while draining stderr concurrently. Do not replace staged fetch with an
   ordinary fetch: selected URL/refspec validation, imports, and transaction
   safety must remain intact. Stream draining must avoid pipe deadlocks.
3. Core operations publish events through an optional reporter/callback;
   invocation code owns terminal policy and one display. Reuse existing
   reporting primitives where practical instead of adding per-command spinners.
4. Connect **current dispatch paths**: `refresh_workspace_repos`, direct clone,
   mirror clone/prefetch, registry initial fetch, and `repo fetch --all`.
   An unused legacy sync helper is not evidence that live sync is covered.
5. Show sanitized repo identity and actual phases. Before Git reports detail,
   say `Fetching <repo> · waiting for Git · <elapsed>` rather than guessing DNS
   or authentication. Preserve enumeration counts and receiving counts/bytes/
   rate when available; show percentages only for the phase Git measures.
6. Batch workers publish separate states into one renderer. Show completed/total
   and active identities, including the final slow worker. Include import,
   index/repack, lock wait, and clone propagation after the network phase.

This is the first independently reviewable behavior change. It addresses
S1-S3 and N1-N6 rather than merely animating the existing static label.

### Slice 2: Safety checks, locks, and lifecycle operations

Cover S4, M1-M4, M8-M9, and registry deletion with the same contract. Name
workspace/repo and active safety/measure/copy/delete/restore/checkout phase.
For lock contention, identify resource and elapsed wait; show the recorded PID
only as diagnostic metadata, not proof the process is alive. Keep existing lock
timeouts, short critical sections, confirmation rules, and fail-closed checks.

Report copied files/bytes without an extra full pre-scan solely to invent a
percentage. Reuse existing size information when available. Remove transient
status before a prompt or permanent result line.

### Slice 3: Reads, integrations, and auxiliary subprocesses

Cover N7-N8, M5-M7, and R1-R6 using named delayed phases and completed/total
where available. Avoid scanning Go modules twice if a small internal refactor
can retain discovered paths. Keep setup/exec child output inherited; command
labels are useful, but a terminal overlay must yield to child output and input.

### Slice 4: Contingent startup and completion work

Measure R7 before adding broad startup machinery. Short-circuit pager fallbacks
when an explicit choice is already known. Preserve silent completion and local
reads. Arbitrary subprocess/network timeouts and credential handling changes
are separate policy decisions: this audit establishes missing feedback, not a
safe universal deadline. Activity feedback alone does not resolve a true hang.

### Efficient implementation ownership

Do slice 1's shared event/runner interface first. After it stabilizes, three
agents can work independently on network callers, lifecycle/filesystem callers,
and read/setup callers, each in distinct files. One coordinator owns the shared
renderer/runner and integration checks. Use targeted tests per slice before
running the required broader repository checks; avoid overlapping edits to
`git.rs`, `progress.rs`, or the same CLI file.

## Acceptance and verification plan

These are proposed implementation checks; no runtime tests were run for this
documentation-only source audit.

- Controlled fake Git/gh with a quiet start longer than one second: meaningful
  named feedback appears before one second and continues to show activity.
- Fast work: no transient progress or unconditional start header left behind.
- Delayed raw enumeration and transfer events: retain truthful counts/bytes;
  handle CR, newline, and partial stderr; expose actionable diagnostics.
- Staged fetch still returns the exact porcelain/ref data and preserves safety
  behavior. Large simultaneous stdout/stderr must not deadlock.
- Mixed batch workers: one display, correct completed/total and active slow repo;
  stable final result ordering and partial failure behavior.
- Held config/metadata/import locks: delayed resource feedback, clean release,
  existing timeout, and no stale timer output after success/failure.
- Controlled slow scan/copy/deletion: label actual phase, update available
  counters, preserve GC/quarantine/recovery semantics. Exercise EXDEV fallback
  through the existing copy helper rather than requiring two mounted disks.
- Quiet setup versus verbose/interactive child: useful command label without
  corrupting inherited output or taking over input.
- TTY, redirected stderr, and `--json`: stdout JSON stays one unchanged document;
  redirected diagnostics use plain lines without cursor controls. Decide and
  document whether JSON with TTY stderr gets animation or plain diagnostics.
- Success, failure, Ctrl-C, and broken pipe: clear transient state, restore cursor,
  preserve exit/result contracts, and never print progress after completion.
- Exercise the actual CLI routes, including workspace-local direct transport and
  `repo fetch --all`. Reuse platform PTY patterns in
  `crates/wsp/tests/pager_behavior.rs` for interactive coverage where applicable.
- Follow repository testing/review guidance, prove regression tests fail when
  the covered behavior is broken, run required checks, and record an inspectable
  terminal UX proof before a behavior-changing PR.

Track this inventory in a focused GitHub issue when the maintainer chooses to
file it; no issue was created or external message sent by this audit.


## Implementation outcome

The inventory is implemented in the dedicated `codex/progress-feedback`
worktree. See the [implementation record and verification results](2026-10-08-progress-fix-plan.md#implementation-record).
The reported fetching gap was reproduced with the original binary, then fixed
and checked while the child was still blocked. Full repository CI and both
local offline smoke dialects passed. Native Windows/Linux runtime coverage
remains with platform CI.
