# Legible slow operations: implementation plan

**Date:** 2026-10-08
**Status:** Implemented in the dedicated `progress-feedback` worktree and split
into independently validated stacked PRs.
Integrated verification passed. Astra medium approved the plan
before implementation.
**Inputs:** [Progress inventory](2026-10-08-progress-audit.md),
[design tenets](../../design-tenets.md), [architecture](../../ARCHITECTURE.md),
and repository testing/review guidance.

## Outcome and boundaries

When wsp waits on its own work for more than a second, users can see the active
operation, the repository/workspace where relevant, and ongoing activity.
When Git supplies measured progress, show that detail. A fast local operation
leaves no transient output. Cover the current CLI routes and every inventory
item; fix the reported fetching experience first.

Keep existing commands, JSON schemas, selected transport, offline bootstrap,
partial failure/resumption, ref transaction safety, confirmations, and child
input/output behavior. No new flags, command names, network retries, guessed
network phases, universal timeouts, or dependencies are planned. A visible
activity indicator describes a wait; it does not claim a stalled child is
making progress. Follow-on performance refactors get separate changes.

## Decisions to implement

### One session and explicit events

Use one progress session per CLI invocation, with scoped operation handles that
can be cloned into workers. Begin observation before slow context resolution;
completion and early help paths remain separate. Each handle reports operation
identity, current phase, available measured counters, and completion/failure.
The session retains the command's work elapsed time across fast phase changes:
restarting a 500 ms timer for every small phase would hide a long command.

Keep event data/optional observer in `wsp-core`; put terminal rendering and mode
policy in `crates/wsp/src/progress.rs`. A introduces the boundary additively;
migrate the existing renderer and cursor restoration as callers move in A-C.
Every live route uses one renderer during migration: leave a legacy route intact
until its replacement is wired, and remove the legacy core renderer by the
network milestone. Suspend the new session with acknowledgment around legacy
renderer scopes so both renderers cannot run within the same invocation at once.
No new core callback writes terminal frames. Existing
non-observing core entry points continue through no-op observation where that
saves churn. Observed entry points used by the binary must be public across the
crate boundary; helpers remain `pub(crate)`. Inventory and explicitly migrate
the few public progress-specific APIs. Do not add a general task framework.

Suggested event shape: begin/update/finish for a scoped operation, with an owned
resource label, phase, and optional count/total/bytes/percent. Choose the smallest
typed representation that fits actual callers. A percentage always belongs to
one measured Git phase, never to total fetch or total command completion.
Publish observation outside operation locks and never hold progress state while
waiting on a child, writing terminal output, or executing a callback. Observer
delivery is nonblocking: coalesce numeric/activity updates into bounded latest
state rather than building an unbounded event queue. Pipe-draining threads never
write terminal output or wait for a renderer acknowledgment.

### Output and timing policy

| Mode | Policy |
| --- | --- |
| Human invocation, stderr TTY | Reveal after 500 ms of active work. Tick activity at roughly 100 ms and elapsed seconds at least once per second, even without worker events. Render one bounded-width line. |
| `--json`, including TTY stderr | Plain stderr diagnostics, no ANSI/spinner. Stdout remains exactly the existing structured document. |
| Redirected/non-TTY stderr | Same plain diagnostic policy. Reveal slow work after 500 ms; coalesce numeric changes, deduplicate phases, and emit at most one progress line per second across the session. Repeat a quiet wait after 10 seconds. Keep final results and actual warnings distinct from this progress budget. The first wait message still arrives within a second. |
| Fast work | Cancel before reveal. No transient start message; existing final results, safety warnings, and necessary side-effect records remain. |
| Prompt, pager, or inherited setup/exec child terminal | Clear and suspend transient rendering with acknowledgment before handoff. Preserve input ownership and visible prompts. Resume only after handoff ends; known user-wait time does not count as wsp work. |
| Authentication-capable Git child with a controlling terminal | Use the conservative append-only policy below, even in human TTY mode; do not infer terminal ownership from parsed prompts. |
| Shell completion | No progress output. Keep its protocol silent and local. |

Labels use repository identity/shortname or workspace name, not raw remote URLs
or arbitrary command arguments. Escape control characters and truncate by display
width, including narrow terminals. For a batch, show completed/total plus a
fairly rotating active repo and phase; additional active count prevents claiming
only that repo is running. After all workers finish, switch to propagation or
publication instead of leaving a 100% fetch bar over unfinished work.

Existing permanent warnings/results must go through a session-aware output gate:
clear the transient line, write the permanent message, then redraw if still
active. A frame carries a generation; the output gate rechecks generation and
suspension after acquiring output serialization, immediately before writing.
Suspend/finish invalidates pending frames under that same gate, clears the line,
and acknowledges completion before a prompt/final result is written or a child
terminal starts. Never take the output gate while holding progress state.

Convert affected CLI/core feedback sites explicitly; a terminal mutex alone
cannot coordinate unmodified `eprintln!` calls. Keep existing semantic warning/
safety messages; remove unconditional operation-start headers only where the
session supplies the replacement. Main explicitly finishes and joins the
renderer before final output and every normal `process::exit` path; Drop is a
fallback, not the only shutdown mechanism. Ignore late worker events. Ctrl-C
preserves existing exit 130 and cursor restoration without waiting on renderer
locks. Shutdown must not depend on worker events or introduce a lock cycle;
ordinary OS output backpressure is not a promise of bounded terminal I/O.

### Captured Git and gh processes

Extend the existing sanitized Git runner rather than introducing different
transport/config rules. Drain stdout and stderr concurrently. Capture porcelain
stdout exactly as today for ref parsing, and parse stderr incrementally for
progress events. Request Git `--progress` on observed clone/fetch paths.
Preserve environment sanitization, scoped user transport configuration, URL
rewrites, staged fetch/imports, exit handling, and error details.

Handle CR, newline, split UTF-8 reads, and unterminated stderr. Recognize actual
Git progress records: enumeration counts, measured percentages, receiving
counts/bytes/rate, and resolving/checking-out phases. Unknown phases retain a
truthful `waiting for Git` activity label. Do not infer DNS or authentication.
Keep stderr for the existing failure response; bound parser pending buffers so
an arbitrary unterminated record cannot grow a second unbounded copy.

Non-progress diagnostics are sent to the output owner independently from numeric
progress, including partial records after a short flush deadline, with terminal
controls neutralized and credential-bearing URL userinfo redacted in the live
display. The bounded diagnostic delivery path must never backpressure capture:
if it fills, retain the complete original in the existing error capture and
coalesce a notice that live diagnostics were suppressed. Incomplete URL candidates
are withheld or conservatively redacted across read/flush boundaries, never
printed as an apparently safe prefix before `@` arrives.

Before B, exercise a small runner feasibility gate for authentication. Preserve
stdin explicitly for each existing route: the current captured `.output()`
runner uses null stdin; the human TTY clone `.spawn()` path inherits stdin.
No observer migration silently changes those settings or SSH/askpass policy.
Because `/dev/tty` interaction is not observable through stderr, any Git child
that may access a controlling terminal uses append-only status/diagnostics,
with no cursor hiding, line erasure, or in-place redraw while it runs. Display
measured bars/counts in bounded appended lines; quiet waits use the plain-mode
cadence. Labels say waiting, not progressing. This may interleave appended text
with a terminal prompt, but must never erase it or consume its input. Ordinary
auth-free local stages can resume in-place rendering after the child exits.
Apply that policy session-wide: track all concurrent children that may access
the controlling terminal, and resume in-place rendering only when none remain.
Do not match English prompts to decide policy. Test a helper that writes an
unterminated prompt directly to the controlling terminal and reads a reply,
plus askpass separately. If these checks fail, B remains blocked on a concrete
terminal-ownership policy rather than shipping heuristic prompt detection.
Arbitrary capture semantics outside the runner remain unchanged.

Optional observer failure or a closed diagnostic sink disables observation;
stdout/stderr continue draining and Git runs normally. A blocked sink is isolated
from the drainers by bounded/coalesced delivery. Only a failure in required
capture, child stdin feeding, or safe process supervision closes pipes and
kills/reaps the direct child as needed, producing an operation error. Keep
display-error handling distinct from child/capture-error handling. Maintain
current Ctrl-C child behavior. No new platform-specific process-tree
cancellation is promised by this change.

## Delivery sequence and ownership

Each row is an independently reviewable patch/PR. Coordinator owns shared files
and integration. Parallel workers begin only after A's interface is agreed;
B lands before C depends on its runner. Rebase subsequent patches on their
dependencies rather than reviewing one giant behavior change.

| Patch | Inventory coverage | Work and ownership | Exit gate |
| --- | --- | --- | --- |
| A: observer + renderer | S1; output/timing foundation | Coordinator: additive core event boundary, CLI renderer, main lifecycle, permanent-output/handoff gate; migrate existing routes without an intermediate loss or double display. Add controlled-clock tests and one quiet-operation wiring test. | Fast hidden, slow named within 1s, continuous owned-TTY activity, bounded plain output, no late frames, usable prompts/children. |
| B: streaming runner | S2-S3 | Coordinator: authentication feasibility gate, `git.rs`, parser and concurrent drains; staged-fetch/repack/index instrumentation. Separate required capture from optional observation. | Quiet start and raw counts visible; both pipes drained under load; sink failure harmless; authentication input, porcelain/ref safety and failure semantics unchanged. |
| C: network workflows | N1-N7; N6 lock integration finalized in D | Network worker: `transport.rs`, `cli/fetch.rs`, registry discovery/registration, mirror/direct add callers. Coordinate edits in `add.rs`, `repo.rs`, `template.rs`, `doctor.rs` with their later owners. | Real current fetch/sync refresh routes, direct additions, prefetch, initial registry fetch, and `--all` propagation all show phases through completion. |
| D: locks and safety | S4, M1, M3, M4, M8, N8 | Lifecycle worker: `filelock.rs`, workspace/workspace_add observation, deletion checks, rename/rollback, sync apply/continue/abort. Coordinator supplies import-lock observer in `git.rs`. | Contention visible with resource/elapsed; safety fetch before confirmation named; checkout/deletion/sync work named; guards and results unchanged. |
| E: filesystem lifecycle | M2, M9; registry deletion | Lifecycle worker after D: GC size/copy/delete/restore and registry-removal observation. Reuse current GC/quarantine helpers. | Slow measurement/EXDEV copy/deletion observable; files/bytes where available without extra denominator scan; recovery safety preserved. |
| F: reads and auxiliary work | M5-M7, R1-R6 | Read/support worker: status/PR, diff/log, size listing, doctor/discovery, language scans, gh setup detection, quiet setup command labeling. Sequence shared CLI files after C/D. | Every named local/API phase observed; reads remain local; setup output/input ownership preserved. |
| G: startup and completion closure | R7, all no-special-gap command groups | Coordinator: coarse delayed context/config/filesystem probe phases before dispatch; pager preparation phases. Completion stays silent. Verify low-risk commands inherit coverage without unconditional messages. | Delayed context and pager probes name the wait; embedded help and completion retain early dispatch; full inventory reconciled. |

Start A+B serially for a stable interface and correct pipe semantics. Then run
network C, lifecycle D/E, and reads F in parallel with explicit file ownership.
No two workers edit `git.rs`, `progress.rs`, `workspace.rs`, or the same CLI
module simultaneously. G and final coverage reconciliation belong to the
coordinator. A worker sends proposed shared-file changes to its owner rather
than applying overlapping edits.

Before dispatch, record a concrete file-owner register: coordinator owns
`git.rs`, core/CLI `progress.rs`, and `main.rs`; network owns `transport.rs`,
`cli/fetch.rs`, `mirror.rs`, and initially `cli/repo.rs` / `cli/add.rs`; lifecycle
owns `filelock.rs`, `workspace.rs`, `workspace_add.rs`, `gc.rs`, and CLI deletion/
recovery/rename/sync; reads owns status/PR/diff/log/list/setup, setup runner,
language/discovery, context/pager. Network requests lifecycle-applied adapters
in workspace files; lifecycle's registry-deletion edits wait for network's
`repo.rs` handoff. Read/template/doctor/shared add edits are dependent work
after their network/lifecycle owners finish. Reassign ownership explicitly
instead of treating all of F as immediately parallel.

**First useful milestone:** A+B+C fixes the reported network experience and
retires the legacy renderer. D-G are not prerequisites to landing it. S4 and
later command gaps remain open until their patches land; do not close the entire
inventory on delivery of this milestone.

Avoid piggybacking duplicate Go-scan elimination, eager pager optimization,
credential/timeout policy changes, or unrelated bug fixes. Record those as
follow-ups; this plan instruments existing behavior. R7 is not deferred solely
because slow mounts are hard to reproduce: a coarse context phase covers
waiting, while optimization remains measurement driven.

## Required verification

Use the repository's [testing-and-review guidance](../../../.claude/skills/testing-and-review/SKILL.md):
choose the cheapest test that reaches the bug, and demonstrate each new
regression test fails when its covered behavior is removed.

1. **Renderer logic:** controllable clock/sink; fast finish, cumulative short
   phases, quiet heartbeat, phase changes, long Unicode labels, suspended
   intervals, permanent output, late events, and closed stderr. Pause a frame
   between snapshot and output, then suspend/finish and verify it cannot appear
   after prompt/final output. Unit tests avoid fragile sleep assertions for pure
   policy.
2. **Real CLI wiring:** a Rust fixture child/fake Git or gh emits scripted
   events and blocks until released. Read stderr while it is still blocked;
   observing a final transcript is insufficient proof feedback appeared before
   completion. Use a small number of real-time deadline tests with generous
   platform scheduling margins, alongside controlled-clock policy tests.
3. **Pipes and parsing:** simultaneous large stdout/stderr; mixed CR/newline,
   split Unicode, non-percentage enumeration, transfer measurements, a partial
   diagnostic, bad/unknown records, child failure, reader error, and cleanup.
   Flood progress and verify bounded live output with exact captured stdout;
   split credential URLs across reads and partial-flush deadlines. Inject observer
   failure/closed stderr and assert Git still publishes the intended refs. Block
   and later release the sink while proving the child still completes. Assert
   required stdout and exit/error contracts, not merely exit success.
4. **Behavioral route matrix:** one and multiple repos; mirror/direct transport;
   `repo fetch`, `repo fetch --all`, `sync` normal/continue/abort/dry-run;
   registry bare clone plus initial fetch; `new`; host and local `repo add`;
   `rm` checks before prompt; `repo rm`; `recover`; locks; EXDEV helper; ordinary
   local reads; context/pager; quiet and verbose setup. Representative shared
   mechanism tests are reused, with focused routing tests at each distinct
   boundary rather than one large repetitive test per command.
5. **Modes and handoff:** human TTY, redirected stderr, JSON with/without TTY
   stderr, stdin TTY/non-TTY, prompts, inherited child output/input, and pager.
   Confirm JSON schemas/results unchanged and no ANSI in plain modes. Exercise
   direct controlling-terminal prompt/input and askpass independently; preserve
   each route's null/inherited stdin setting.
6. **Existing safety coverage:** retain workspace-local transport, fetch ref
   casing/import, crash/resumption, sync conflicts, GC, pager, and broken-pipe
   regressions. Run targeted suites for touched behavior, then the required repo
   checks. Cross-platform pipe behavior needs Windows runtime CI, not only a
   successful cross-compile. Reuse existing Linux/macOS PTY test patterns;
   document any Windows terminal UX gap explicitly.
7. **Review and delivery:** `just fix`, `just ci`, and applicable existing
   shell/PowerShell offline smoke checks; do not invent flags to test progress.
   Record terminal UX proof for quiet fetch start, transfer, final slow worker,
   direct add, and handoff. Each behavior PR needs the required `whatsnew` and
   `ux-proof` blocks. Regenerate CLI contract docs only if their inputs change.

Completion means every inventory ID has an implemented observation path or a
documented intentional interaction policy, representative live-command tests
have passed, and the source audit links to the landing patches. Planning alone
does not close the reported bug.

## Review record

Astra medium reviewed the draft and requested four contract fixes: isolate
optional observation from required capture failure; avoid inferred authentication
handoff; prevent stale frames after suspend/finish; and bound plain-mode output.
All four are incorporated above, together with split-URL redaction tests, concrete
file ownership, public observed-entry-point visibility, and the A+B+C milestone.
Astra medium's focused recheck concluded **ready for phased implementation**,
with two implementation guardrails now explicit above: terminal ownership policy
applies to the entire session across concurrent children, and legacy/new
renderers cannot overlap within an invocation. No further broad review is needed.
No source changes, issue creation, or external messages are part of this
planning task.


## Implementation record

All seven slices are integrated. The binary owns rendering; core operations
publish optional process-scoped events. Installation occurs at the invocation
boundary and reaches existing scoped workers without changing core signatures
or JSON data. Library callers can omit the observer. Result rendering, pager
lookup, automatic GC, and advice use sequential observation sessions when they
run after command completion; user-owned child runtime remains suspended.

Git clone, fetch, repack, index-pack, and ref transactions drain their required
streams independently from live display. Concurrent stdin feeding prevents
large input from deadlocking behind stderr. Native enumeration and percentage
records carry authoritative measurements separately from formatted labels.
Batch fetch, PR, and sync counters use the same typed metadata. Narrow rendering
reserves measurements and elapsed time before shortening resource names.

Locks, safety checks, filesystem traversal/copy/deletion, recovery, rename,
workspace synchronization, reads, discovery, doctor, setup, context resolution,
and pager lookup have named scopes. Nested scopes count once per worker.
Terminal handoff occurs before prompts and inherited children. Authentication-
capable Git children use append-only output for the entire concurrent session.
JSON and redirected stderr stay plain. No retries, command flags, transport
fallbacks, schema changes, or new dependencies were introduced.

Independent code and security reviews found and prompted fixes for warning
classification, partial credential URL display, resource labels, pager session
coverage, setup argument disclosure, and unnecessary TTY stdout capture. A
focused source recheck found no remaining blockers. Final runtime checks passed.

Verification includes deterministic renderer clock/sink tests with mutations,
real blocked clone/fetch and gh/PR discovery handshakes, large simultaneous
stdin/stdout/stderr, optional observer failure with exact resulting refs,
controlling-terminal authentication held until progress is visible, and a
separate inherited askpass-helper environment fixture. The latter verifies
helper environment propagation using a fake SSH transport, not OpenSSH's
server-specific authentication protocol. Windows/Linux type checks supplement
macOS runtime tests; native Windows runtime still requires CI.

The machine's configured Cargo/npm firewall registry was unavailable. Checks
use temporary cache/config wrappers and the public pinned model dependency,
without changing user or repository registry settings. Existing localhost and
PTY fixtures require execution outside the restricted sandbox.


### Final verification results

- `just ci`: passed, including normal/codegen lint, Windows/Linux type checks,
  dependency audit, the full workspace test suite, formal models and expected
  fault mutations, 31 crash/recovery tests, offline release smoke, and generated
  CLI-contract freshness.
- `just smoke-powershell`: passed against the same release artifact on macOS.
- `git diff --check`: passed. Dedicated worktree changes leave the shared
  checkout's source untouched.
- The original binary failed the strict blocked-clone regression, showing only
  its static URL header. The fixed binary passes before the child is released.
  A separate baseline fetch reproduction emitted no enumeration; the fixed
  release showed `slow · Git fetch · Enumerating objects 123, done. · 0/1 (0s)`
  while the child remained blocked, then completed successfully.
- Targeted mutations proved renderer timing/gates/width, worker-leaf selection,
  Git partial URL withholding, measurement details, required stdout capture,
  optional observer failure, active authentication prompt protection, inherited
  askpass helper environment, registry/PR discovery, filesystem counters/copy,
  language scans, and setup handoff. Protections were restored and checked green.

The smoke assertions now validate actual removal/recovery results and plain
redirected stderr instead of requiring unconditional start headers. The
PowerShell fixture handles intentionally empty JSON stderr and reports the
source location of unexpected failures. Temporary fixture commits disable
signing rather than requesting a human signing key.

Native Windows/Linux runtime checks and server-specific authentication variants
remain platform CI responsibilities. No installed user binary was replaced.
A tooling follow-up is suggested for the unavailable configured Cargo/npm
firewall registry encountered on this machine.


### Review stack and recorded demos

1. [Network and rendering foundation](https://github.com/jganoff/wsp/pull/205):
   optional observer, streamed Git feedback, batch counts, and the prompt/child
   adapters required by the invocation-wide renderer. Includes
   `docs/demos/progress-network-demo.py`, its actual `.cast`, and a GitHub GIF attachment.
2. [Lifecycle and filesystem waits](https://github.com/jganoff/wsp/pull/206):
   named locks/safety/mutation phases and observed filesystem counters. Includes
   `docs/demos/progress-lifecycle-demo.py` and recordings of lock contention,
   rename, removal, and recovery.
3. `codex/progress-reads`, based on the lifecycle branch: named read/PR/doctor/
   discovery/language/setup phases and final inventory records. Includes
   `docs/demos/progress-reads-demo.py` and recordings of a fast local status and
   a blocked PR lookup followed by successful status.

The first two intermediate trees passed their own lint, full tests, and release
validation. The second also passed the PowerShell smoke target. Each layer had
parallel code/security reviews before its agent-signed commit. The initial
split review identified a dependency: all prompt and inherited-child handoffs
must ship with the foundation renderer. Those adapters were moved into layer
one and its standalone checks/reviews repeated successfully.

Text recordings and fixtures stay in the repository; rendered media lives in
GitHub PR attachments. Demos were inspected for progress redraws, completion,
compact size, and private output. PR descriptions link immutable source recordings.

## Follow-up: animated rows and Git compatibility

**Date:** 2026-10-09. **Status:** Implemented; local validation and independent reviews passed.

The competing-output fixture demonstrated that parallel native Git processes
can overwrite each other's progress and prompts. Configuration admission is
rejected: wsp must not infer terminal behavior from Git configuration, hooks,
helpers, or executable provenance.

### Prior art checked on 2026-10-09

[Homebrew's Git download strategy](https://github.com/Homebrew/brew/blob/main/Library/Homebrew/download_strategy/git_download_strategy.rb)
disables terminal credential prompts and retains user configuration for helpers.
Its isolated fetch path also disables SSH askpass while preserving agent access.
[Mise's Git adapter](https://github.com/jdx/mise/blob/main/crates/mise-util/src/git.rs)
abandons the progress reporter before CLI clone to avoid hiding password prompts;
its update path captures Git output. Neither establishes universal authentication
compatibility with an animated panel. Wsp adopts explicit terminal ownership and
an opt-in native path, without copying configuration-based admission.

### Production contract

Parallel mode is the default. Wsp owns the display and captures Git output.
Unknown work uses continuously animated compact bars on the left; measured Git
percentages fill the same bars. Repository columns align, and the visible rows
are bounded by the terminal height and an eight-row maximum. Git children use
separate Unix sessions or Windows console isolation with Job Object ownership.
Ordinary Git children and helpers have no controlling terminal in this mode.
This isolates terminal ownership; it does not sandbox custom helper programs.

Native mode serializes Git operations, announces the repository and operation,
clears the panel, and gives Git terminal access until that operation ends.
Terminal passwords, SSH passphrases, host-trust confirmation, and helpers that
open the controlling terminal use this compatibility path. Wsp resumes its
panel afterward. Native Git may be quiet while waiting; wsp cannot animate the
terminal while another program owns it.

Use `--git-progress native` for one invocation, `wsp config set progress.mode
native --global` for a global preference, or `wsp config set
progress.repos.github.com/owner/repo native --global` for a repository override.
Invocation choice takes precedence over repository preference and global default.
JSON always captures and isolates Git to protect its structured output contract.
No automatic error-based retries or silent preference changes occur.

Parallel Git disables terminal prompting and standard GCM interaction. Existing
cached credentials, SSH agents, and nonterminal helpers can work. Browser,
keychain, and hardware interaction remains helper-controlled: detachment does
not guarantee that all external UI is disabled, or that hardware touch will
fail. PIN or confirmation workflows requiring a controlling terminal need
native mode. On Windows, Git's launcher can create a private invisible console;
custom helpers may wait for input there. Parallel mode isolates the caller's
terminal, not every helper-created interaction surface. Such workflows need
native mode; the bounded access check can report a timeout without identifying
its cause. Wsp makes no compatibility decision by reading Git configuration.

`wsp setup --check-access` and `wsp doctor --check-access` opt into one bounded
`git ls-remote --quiet` attempt per remote under parallel policy. Existing clones
supply their actual `origin` and working directory; missing mirror contexts use
a neutral directory. A success proves access on that attempt. Failure and
15-second timeout report an unknown cause and an actionable native retry.
Checks do not retry or change preferences. Git can still invoke helpers with
credential-storage or SSH-trust side effects. Ordinary doctor stays local.
Arbitrary helper diagnostics are not exposed by access checks.

Cancellation owns Git children through their final reap and kills detached
process groups or Windows jobs before exiting. Real-terminal fixtures cover
competing output, native HTTP and SSH prompting, cached helper credentials,
JSON isolation, and descendant cleanup. Hardware fixtures describe the tested
input mechanism rather than claiming physical-device certification.

Verification includes full local CI, both offline smoke dialects, real-terminal
authentication and competing-output fixtures, and parallel code/security review.
Negative controls reject missing cursor handling, repository overrides, and
leaked probe diagnostics. Inspected GIFs are PR attachments; their text casts
remain reproducible fixtures. Windows runtime fixtures create a private console
and verify native access, detached caller-console isolation using console
process membership, a direct child's lack of console, real Git-helper
cancellation, and a failing native negative control. Native Windows authentication UX and physical hardware
authentication still require platform/device validation.
