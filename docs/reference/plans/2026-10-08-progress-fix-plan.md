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

## Follow-up design: animated rows and terminal ownership

**Date:** 2026-10-08. **Status:** Bounded Unix prototype built; evidence below.
Production rollout and an authentication bridge are NOT
approved. Earlier implemented sections remain historical. Prototype code runs
only through an explicit test/prototype target. Passing its gates produces a
recommendation; changing the default runner requires a separate implementation
decision.

### UX contract

Keep the agreed compact eight-cell bar on the left, stable named repo rows,
500 ms reveal delay, and 100 ms animation tick. Unknown progress bounces in the
SAME rows; measured progress fills the same track. No repeated panels, alternate
screen, or new user flags. Preserve overflow, narrow terminals, and final output.

A prompt receives exclusive input/display ownership before it appears. Clear
progress once; coalesce worker updates and defer unrelated diagnostics; restore
current rows after the owner releases the terminal. No input goes to another
repository. Preserve JSON, non-TTY output, authentication configuration, exact
Git stdout, ref transactions, and no automatic retries.

**Scope of the first candidate:** animate through a silent managed fetch; after
supported native terminal interaction starts, keep the display yielded until
the whole managed operation and its terminal users finish. Immediate resumption
after an answer while that same Git process continues is a later goal requiring
an explicit prompt-completion protocol. Do not infer completion from quiet
output, a newline, echo settings, or elapsed time. Keep current timing semantics;
do not claim to measure native input-wait time separately.

### One owner, with rendering kept optional

The existing renderer already draws animated bars. The missing piece is reliable
terminal ownership: `capture_with_progress` holds `progress::external()` for the
entire child because `/dev/tty` can bypass its stdout/stderr pipes.

A required runner/coordinator owns child lifetime and fallible terminal grants.
The optional progress observer is only a client. Reuse the output gate and
frame-generation validation for clear/acknowledge, but do not use today's
`progress::suspend()` as proof of a successful grant: observation can be disabled.

The display states are rendering, yielded to one identified child, and stopped.
Before forwarding an eligible child's terminal interaction, invalidate pending
frames, clear under the output gate, and acknowledge ownership. Queue other
eligible children FIFO; release only when the owner and terminal-using descendants
are accounted for. Never hold renderer/state locks while waiting for input or
child completion. Stop/cancellation invalidates queued grants and late releases.
Pipe drainers remain independent of renderer and terminal backpressure.

One native PTY cannot distinguish sibling readers in the same Git operation.
A managed profile must prove a single interactive reader at a time; shared
operation identity is not evidence. Any future cooperative bridge needs unique
prompt-request IDs and explicit nested lease borrowing, not blanket reentrancy.

### Bounded feasibility prototype

Compare two Unix adapters before choosing a production architecture:

| Candidate | What it could provide | Unproven or costly boundary |
| --- | --- | --- |
| Private controlling PTY, with separate stdout/stderr pipes | Capture direct terminal interaction before it reaches the user's screen. | Stdin mapping, no-echo input forwarding, sessions/signals, descendants, and cleanup. Ordinary full-PTY spawn APIs merge streams and are not a drop-in. |
| Same-session process group with OS job-control handoff | Native terminal input without a byte proxy; SIGTTIN can expose a read-first child. | TOSTOP affects unrelated background jobs; ignored SIGTTOU and descendant groups can bypass supervision. Not approved as a transparent default. |

The private-PTY candidate must preserve null/data stdin and isolate inherited
terminal stdin without leaking original terminal descriptors. It must not create
new prompting capabilities when the original invocation had no controlling
terminal. Isolation is for accidental I/O coordination, not a sandbox against
same-user code reopening an explicit original tty path or external daemons.

**Eligibility is an output of the prototype, not an assumed allowlist.** Resolve
it from the actual sanitized child environment and flattened transport config.
Neither a remote URL, an executable basename, nor an empty pipe proves safety.
Use a positively demonstrated interaction contract. Private-tty output alone
misses stderr-prompt/tty-read and read-without-output helpers; those paths must
use the existing runner unless a tested adapter supports them. Do not guess
which waiting process should receive input. Unknown custom helpers, multiple
native readers, JSON, non-TTY, and Windows initially keep the existing runner.
Any unmediated terminal child suppresses redraws across the invocation.

Do not add an askpass broker, change GIT_ASKPASS/core.askPass/SSH_ASKPASS selection,
force SSH_ASKPASS_REQUIRE, or modify per-worker clocks in this slice. Preserve
credential helpers, GUI/browser/keychain flows, BatchMode, and no-prompt settings.
The existing Git config/sanitization path remains authoritative. Pick a runner
before spawn; failures after launch are errors, never a reason to repeat Git.

The required coordinator must outlive optional observer failure. Current
`main.rs` exits immediately on Ctrl-C and assumes shared foreground signal
delivery; an isolated runner cannot ship with that assumption. Define Ctrl-C,
Ctrl-Z/continue, resize, spawn-registration races, EOF, and descendant cleanup.
Restore terminal modes, reap supervised processes, and preserve exit 130.
An optional display failure disables display; required input/ownership failure
terminates the affected managed operation safely. Bounded queued native output
must never spill secrets to disk or deadlock pipe capture; overflow is a required
broker failure. Native input/output never enters progress events or error logs.

### Acceptance and delivery

1. **Prototype only:** use existing build-system targets or add a focused target.
   Demonstrate exact binary stdout, simultaneous stdout/stderr draining,
   null/data/terminal stdin, absent controlling terminal, output-first native
   prompts, stderr-prompt/tty-read, read-first helpers, competing child prompts,
   same-child readers, optional observer failure, cancellation, resize, job
   control, and cleanup. Unsupported cases must be selected for the old runner
   before launch. Test actual local Git HTTP and OpenSSH endpoints as well as
   deterministic helpers. No real credentials or network retries.
2. **Choose or stop:** publish a concrete effective-config eligibility table and
   adapter comparison. A fixture-only success is insufficient. Stop if a useful
   real-world profile cannot be supported without understanding arbitrary helper
   code, changing user auth policy, disrupting unrelated jobs, or writing a
   general terminal emulator. Re-review the result before production code.
3. **Implementation only after the gate:** integrate the smallest proven runner
   and coordinator, with parallel code/security reviews and `just ci`. Windows
   remains unchanged until independent console/capture/lifecycle evidence exists.
4. **Visual proof:** record silent fetching with multiple cursor traversals in
   the same rows, then safe native handoff and restoration after child completion.
   Include two competing supported prompts. A terminal-screen assertion verifies
   row footprint and input routing; counting different bar strings is insufficient.
   A future cooperative bridge must separately prove prompt/answer/resume while
   the SAME fetch remains alive. Keep casts/fixtures in Git, GIFs in attachments.

Pure policy tests use injected clocks and deterministic handshakes. Real PTY
tests use generous watchdogs solely to bound a missing handshake. This planning
change needs diff/document checks, not a runtime test suite.

### Independent review and rejected shortcuts

An independent agent was requested as Astra with high reasoning. The first
combined PTY/askpass draft received a no-go for production and a go for a bounded
prototype. It identified four blockers: undetectable native input paths,
indistinguishable sibling readers, changed OpenSSH askpass selection, and
required ownership/cleanup coupled to the optional observer. The revised scope
addresses them with explicit eligibility gates, separate runner lifetime, and
deferral of the authentication bridge and input-wait timing changes. A second
review returned GO for this bounded prototype with no remaining technical
blockers to the experiment; production implementation remains unapproved.

Reject prompt-text matching, unconditional redraw over Git, extra appended
snapshots, full-PTY machine-output capture, and interactive retries. The existing
slow-operation tenet already requires motion; no new tenet or guide is needed.

References: [Git credentials](https://git-scm.com/docs/gitcredentials),
[OpenSSH input selection](https://man.openbsd.org/ssh.1),
[PTY semantics](https://man7.org/linux/man-pages/man7/pty.7.html),
[terminal job control](https://man7.org/linux/man-pages/man3/termios.3.html), and
[mise interactive ownership](https://mise.jdx.dev/tasks/running-tasks.html).
Mise's declared interactive-task ownership is inspiration, not proof of
transparent handoff for arbitrary Git authentication.

### Prototype evidence and decision

Run `just terminal-probe` for the experiment matrix and
`just terminal-probe-record` for the actual renderer with two live, controlled
terminal children. The latter writes a text cast in `docs/demos`, validates it
with a terminal emulator, and renders `/tmp/wsp-terminal-prototype.gif`.
Presentation delays are confined to the demo. Process tests advance through
handshakes; watchdogs bound failure, never infer prompt completion.

The experiments live in `crates/xtask/src/terminal_probe`. The demo imports the
existing progress renderer without changing the product runner. On macOS
(Darwin 27, Git 2.56.0, OpenSSH 10.6p1), the private terminal preserved concurrent
256 KiB binary stdout/stderr, null/data/terminal stdin, native output-first
prompts, distinct answers for two queued children, resize, canonical EOF,
observer failure independence, and controlled process-group cancellation.

| Effective fixture configuration | Observed result | Production admission |
| --- | --- | --- |
| Git HTTP, isolated config, empty credential helpers, real loopback 401 challenge | Native username/password prompts use the controlling tty; exact ref output survives authentication. | Candidate for further integration testing; the fixture is not a classifier for arbitrary Git config. |
| Same HTTP profile with `GIT_TERMINAL_PROMPT=0` | No terminal prompt; unauthenticated failure and authenticated success retain native behavior. | Proven only for the explicit fixture configuration. |
| OpenSSH with isolated config, disposable keys, no agent, unknown host | Native trust confirmation uses the controlling tty. | Candidate; does not prove all SSH helper/authentication paths. |
| OpenSSH with a disposable encrypted key, agent disabled | Native passphrase prompt uses the private tty; the binary internal-SFTP exchange succeeds. | Proven only for the explicit fixture configuration. |
| OpenSSH with `BatchMode=yes` | Unknown host fails without prompting; trusted host and test key authenticate. | Proven only for the explicit fixture configuration. |
| Helper writes its prompt on captured stderr, then reads tty; helper reads tty silently | No tty output announces the read. | Unsupported by output-triggered grants. Preserve the existing runner. |
| Multiple native readers inside one child | Tty bytes carry no reader identity. | Unsupported without a cooperative interaction contract. |
| No controlling terminal; JSON; redirected output; Windows; arbitrary custom helpers | No new production adapter is selected. | Existing product behavior remains unchanged. |

The SSH fixture uses an internal SFTP exchange, disables user RC/environment,
forwarding and PTY allocation, and requires the configured session executable's
compiled system `sshrc` path to be identified and absent. Unknown or present
system RC paths skip authenticated SSH tests. This is a test-host precondition,
not isolation for arbitrary installed programs.

**Choose the private-terminal direction for further design; reject global job
control.** The job-control experiment proves that `TOSTOP` stops an unrelated
background writer, while ignoring `SIGTTOU` bypasses it. Without `TOSTOP`, native
output overwrites the progress frame. Read-first handoff and stop/continue do
work, but do not compensate for those compatibility costs.

The recording proves two fixed repository rows, all seven cursor positions,
repeated direction changes, cleared rows before prompts, ordered native
handoffs, resumed rendering after both children exit, and final cleanup.
Truncating the handoff or injecting a row over a prompt makes the screen check
fail. Prompt text in the fixture driver is a deterministic test oracle, never
a proposed production prompt detector.

**This is feasibility evidence, not rollout approval.** The next implementation
decision must establish a useful configuration admission rule without modeling
arbitrary helper code, then a required ownership coordinator independent of
the renderer. Actual user-terminal Ctrl-C/Ctrl-Z propagation, escaped descendant
supervision, Linux runtime parity, cancellation while queued, output overflow,
and spawn-registration races remain unproven production gates. The prototype
must not become the default runner by removing a guard. Native authentication
keeps the display yielded until the operation exits; same-fetch resumption after
an answer still needs an explicit completion protocol.

Code and security reviews found and resolved incomplete screen assertions,
terminal-tail loss, unbounded cleanup, post-reap signaling, inherited local Git
configuration, and account startup-file execution. Both reviewers approved the
bounded prototype after remediation. Local `just ci`, the complete explicit
prototype matrix, the terminal recording check and its negative controls passed.
Windows and Linux cross-checks compile; Linux terminal behavior is not claimed.
