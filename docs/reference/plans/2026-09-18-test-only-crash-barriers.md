# Test-only crash barriers for workspace-local operations

**Status:** Partially implemented. The gated protocol, add/removal/description/guidance,
host registration, refresh, deterministic returned-error seams, and a local/host
concurrency schedule have real-binary coverage. Raw Quint-trace replay and
mid-Git crash supervision remain outside this deliberately bounded test
infrastructure.
**Date:** 2026-09-18
**Related:** [ownership model design](2026-09-18-workspace-local-quint-model-design.md),
[Quint model and execution contract](../../../formal/quint/README.md),
[design tenets](../../design-tenets.md).

## 1. Outcome and scope

Build a dedicated test binary that can acknowledge a named filesystem boundary
and stop there until its parent test either resumes or kills it. The parent must
be able to prove that publication completed, metadata did not yet commit, and a
fresh invocation recovered from the actual disk state. Timing, log matching,
and guesses about when Git finished are insufficient for that proof.

The first implementation covers workspace `add`, adoption, membership removal,
description commits, and add guidance generation. Host preparation and explicit
registration receive barriers needed for opposite-context recovery tests.
Refresh selection receives scheduling/failure points, not a replacement Git
implementation. Workspace creation, rename, whole-workspace removal, GC, setup
internals, and arbitrary commands are outside the initial barrier catalog.

This is testing infrastructure. It adds no product flags, public environment
contract, persistent journal, recovery command, registry, or provenance inside
clones. Real recovery remains an ordinary new invocation. It preserves the
tenets of resumability, explicit side effects, structured CLI output, and short
metadata locks around production operations. An instrumented test deliberately
may hold such a lock while paused.

Here, **persistent boundary** means that a synchronous filesystem operation
completed and its result survives termination of this process while the OS
continues running. It does not mean power-loss durability. Current metadata and
AGENTS.md writes use `NamedTempFile::persist` without file/directory `fsync`.
Hooks must not add flushes, cleanup, or stronger storage guarantees.

## 2. Current implementation and model constraints

The implementation already supplies useful seams:

- `wsp-core/src/workspace_add.rs::add` completes and validates a staged clone,
  acquires the metadata lock, rereads metadata, publishes with non-replacing
  rename, and saves membership while retaining the lock. `existing` separately
  validates and records an adoptable clone under that lock.
- `workspace::save_metadata` writes a temporary file and atomically replaces
  `.wsp.yaml`. A failure after publication must leave the final clone intact.
- `workspace::remove_repos_with_refresh` deletes each clone inside
  `filelock::with_metadata`; that helper saves metadata after the closure returns.
  A process can therefore die after deletion with membership still recorded.
- `agentmd::update` replaces AGENTS.md, then updates CLAUDE.md and installed
  skills. These are several writes, not one atomic guidance transaction.
- `cli/add.rs::ensure_registered` completes mirror clone/fetch before saving the
  registry. `cli/repo.rs::run_add` also has separate mirror and registry phases.
- File locks release on process death. The adjacent lock file remains by design;
  its existence does not establish whether a lock is held.

The executable Quint model is smaller than its design document. It currently
has `crashAfterPublish`; `hostPrepare`, `register`, and `removeSafe` combine
effects that are separate in Rust. It has no process scheduler or partial
guidance state. Its `validMembers` invariant excludes the legitimate interrupted
removal state, and its fixed `slotFor("beta_api")` is `beta-api` even when this
is the first repository, whereas Rust initially allocates `api`.

Consequently, barriers alone do not make arbitrary current ITF traces replayable.
Start with independently specified binary scenarios and the compatible
`alpha_api` publication/crash/adoption journey. Require an explicit model change
or an accurately declared fixture profile before replaying other allocation,
removal, and host-infrastructure traces. Never rewrite expected paths to whatever
the binary produced. Keep unsupported traces labeled **not replayable**.

Two other conformance questions need separate changes: `describe` currently
commits metadata without regenerating guidance, and removal generates guidance
outside a metadata lock. Instrument those behaviors faithfully; do not move
operations or imply the stronger design promise by where a hook is named.

## 3. Build isolation and release exclusion

Use an explicit feature plus a deliberately separate compiler opt-in:

```toml
# crates/wsp-core/Cargo.toml
[features]
test-crash-barriers = []

# crates/wsp/Cargo.toml
[features]
test-crash-barriers = ["wsp-core/test-crash-barriers"]

[[test]]
name = "workspace_crash"
path = "tests/workspace_crash.rs"
required-features = ["test-crash-barriers"]
```

Actual instrumentation is compiled only under
`all(feature = "test-crash-barriers", wsp_crash_test, debug_assertions)`.
Register `wsp_crash_test` with Rust's `check-cfg` configuration. Both crate roots
reject the feature without the opt-in cfg, and reject the opt-in cfg without
the feature. Reject enabled instrumentation when debug assertions are disabled.
Additionally, a small core build-script guard rejects the feature unless Cargo's
`PROFILE` is `debug`; this prevents a release/dist build with manually enabled
debug assertions from becoming an instrumented release. Test builds use the
normal debug profile in a distinct target directory. Do not add a release-like
crash profile. These guards prevent accidental packaging; they cannot constrain
a person intentionally modifying compiler inputs and source.

`cfg(test)` alone cannot do this: Cargo's integration-test `wsp` executable and
its core dependency are ordinary non-unit-test targets. Do not attach the feature
to `test-utils`, a dev dependency, default features, or `codegen`; Cargo feature
unification could otherwise enable it in unrelated builds. `--all-features`
without the explicit cfg intentionally fails and should have a documented
diagnostic directing maintainers to the dedicated test task.

When disabled, hook sites compile away, including argument evaluation. A small
internal macro with enabled/disabled definitions is preferable to a runtime
environment check or constructing events for a no-op function. Cross-crate API
needed by CLI call sites is public only in the test configuration; other helpers
remain `pub(crate)`. Normal artifacts contain neither the protocol parser nor
test environment-variable lookups, networking, error injection, or event strings.

Add `cargo xtask crash-tests`, invoked by `just crash-test`, to set the cfg in the
child Cargo process and build/test under `target/crash-tests`. Set flags through
`Command::env`, preserving supported existing flags without shell interpolation.
Reject conflicting encoded/plain flags instead of silently dropping them. Cargo
builds finish before tests spawn binaries; do not recursively build from tests.
Use `CARGO_BIN_EXE_wsp` from the dedicated integration-test build. The task also
compiles/lints the instrumented configuration and runs release-exclusion checks.
Ordinary release/codegen/dist tasks keep their current feature selection and use
a separate target directory for exclusion checks.

Acceptance requires negative builds for feature-only, cfg-only, release with
feature/cfg, and dist with feature/cfg, including release with debug assertions
forced on. Also run a real default release binary with every test variable set:
it must execute normally, never connect to the test listener, and preserve its
normal JSON/help contract. A binary-string scan can supplement this behavioral
check, but is not the primary evidence. Release CI must never upload artifacts
from `target/crash-tests`.

## 4. Cross-platform parent/child protocol

Use a synchronous, bounded, versioned control connection over loopback TCP using
`std::net`. This works on Linux, macOS, and Windows without `unsafe`, inherited
descriptor conventions, Unix signals for pausing, or Windows named-pipe bindings.
It leaves stdin available for existing prompts and keeps stdout/stderr entirely
within their normal CLI contracts. The parent binds `127.0.0.1:0` before spawning
the child; there is no reserve-port-then-rebind race and no DNS lookup.

In instrumented builds only, startup reads private variables such as
`WSP_TEST_CRASH_ADDR`, `WSP_TEST_CRASH_TOKEN`, and `WSP_TEST_CRASH_SESSION` before
product work begins. With no variables, hooks are inert. Partial/malformed
configuration fails before product mutation. Accept only a literal loopback
address and a bounded port/session/token format. The parent supplies a fresh
cryptographically random capability token for each child through its environment,
never command-line arguments. The channel sends no URL credentials or arbitrary
file contents. Tokens are omitted from retained diagnostics.

Initialization uses a `OnceLock` for one session per process. The child sends a
hello containing protocol version, session, token, its OS PID, and compiled
barrier catalog version. The parent checks it against `Child::id()` and its
requested session before acknowledging and sending the finite barrier selection.
No product operation proceeds until this handshake is complete when enabled.
Other local processes cannot select a fault merely by finding the port. A
malicious process running as the same OS user remains outside the trust boundary;
this is an accidental-cross-talk defense, not a sandbox.

Use length-prefixed JSON messages with a small maximum size (for example 16 KiB),
maximum selection count, strict enum values, and bounded strings. Each reached
event identifies session, process-local sequence number, operation, canonical
repository identity if applicable, symbolic point, per-point occurrence, and
whether the workspace/config lock is held. A validated workspace identifier
maps to a fixture root; no protocol path is accepted as permission to write.

Only selected barriers block. Selection includes operation, repository, point,
and occurrence, so the second repository in a batch cannot accidentally satisfy
the first one's wait. The parent may subscribe to a whole operation's catalog
and automatically acknowledge intermediate points. A selected hook sends
`Reached(sequence, ...)` and waits for exactly one matching `Continue` or an
allowed `Fail` response. The operation cannot advance between notification and
acknowledgment. Reject duplicate, stale, reordered, unknown, or wrong-session
messages. All socket reads/writes and parent waits have monotonic deadlines;
timeouts diagnose a failed test, never release a barrier automatically.

The parent records the authoritative event order. It does not parse stdout to
schedule. Reader threads drain stdout and stderr concurrently with bounded
capture, so a full pipe cannot deadlock a paused command. A failure report
includes the planned schedule, last event, expected event, exit status, and
captured output. Store artifacts outside observed workspace/global/sibling roots.

After acknowledging receipt of a reached barrier internally, the parent kills
the still-blocked process using `Child::kill`, then calls `wait` to reap it before
inspecting files or starting recovery. On Unix this is a forceful process kill;
on Windows it uses process termination. Do not use Ctrl-C, panic, returned
errors, or a graceful shutdown to stand in for a crash: those can run destructors
and remove staging. No `Continue` is sent on the crash path. A normal exit before
the kill or a missing selected event fails the test. Assert termination using
platform exit-status facts; do not require one portable numeric exit code.

A guard owns every child and kills/reaps it on test failure. EOF, invalid control
input, or protocol timeout in an enabled child aborts the process rather than
silently continuing or unwinding through cleanup. This is a harness failure,
never evidence of the requested crash scenario. Parent deadlines are shorter
than child watchdog deadlines. On a barrier that holds a production lock, the
schedule must kill or resume the holder before a peer's real lock timeout.

Initial crash barriers are reached only when Git/setup subprocesses have exited
and been reaped. Killing `wsp` alone does not kill arbitrary descendants on all
platforms. Mid-Git crashes therefore remain unsupported until the harness owns
process groups on Unix and a job object or equivalent safe supervisor on Windows.
Do not claim `Child::kill` supplies that guarantee. A future Rust Git test helper
can provide controlled staging interruptions with explicit child ownership.

Instrumented subprocess builders should remove the private test variables from
Git, hooks, setup, and other child commands using `Command::env_remove`; do not
mutate the global environment, which is unsafe in threaded Rust 2024 code.
Initialization additionally rejects a descendant trying to reuse the parent's
session/PID association. Audit all subprocess paths used by the test scope.

Loopback is a test-control capability, not a product capability. Confinement
profiles must explicitly permit that connection and attribute it separately.
If a platform sandbox cannot allow loopback control, that profile is unsupported
until an equivalent inherited-pipe transport exists; do not weaken the sandbox
or call the test confined because global paths were merely absent.

## 5. Barrier catalog and exact boundaries

Point names are versioned test protocol, independent of Rust function names.
`clone_published` retains the name already illustrated by the normalized trace
schema. Events below are per repository unless marked invocation-wide. A
post-operation point only fires after success. Parent-side assertions establish
what disk contains; the child event is evidence of location, not an oracle.

| Point | Placement and lock state | State at acknowledgment |
| --- | --- | --- |
| `add_admitted` | CLI host path after `existing` returns absent, before `ensure_registered`; no metadata lock | Host eligibility observed; no effects from this admission yet. A later peer win does not undo authorized host effects. |
| `stage_created` | Immediately after `.wsp-add-*` tempdir creation, before invoking Git; no lock | Empty owned staging directory exists; final path and membership unchanged. |
| `clone_staged` | After clone/branch setup and `validate_clone`, immediately before publication lock acquisition | Complete ordinary staged clone exists; final slot not yet owned by this attempt. |
| `add_rechecked` | After latest metadata/branch checks and destination classification, before publish/adopt; metadata lock held | Latest selected member/path facts are captured; no publication by this attempt yet. |
| `clone_published` | Immediately after successful `publish(&staged, &destination)`, before `record` or metadata save; same lock held | Complete clone is at final path; its membership has not committed. Staging wrapper may remain. |
| `adoption_validated` | After validation of existing compatible destination, before `record`/save, in both adoption branches; lock held | Preexisting clone is untouched and unrecorded; there was no publication by this attempt. |
| `membership_committed` | Immediately after successful metadata `persist`, inside the same lock scope, for add/adoption | Member and recorded directory are in `.wsp.yaml`; extras/guidance have not run. |
| `member_already_present` | After successful existing-member validation before returning that disposition | Existing clone and metadata preserved; distinguish unlocked early `member` inspection from locked publication recheck in event context. |
| `remove_preflight_complete` | After snapshot/safety/refresh phase, before per-member metadata lock | Safety checks finished or force policy applied; peer metadata updates remain possible. |
| `remove_rechecked` | Inside `with_metadata` after existing ownership/mapping/branch rechecks, before `remove_dir_all` | Membership and directory still present, unless force accepted an already-missing clone. |
| `clone_deleted` | Immediately after successful `remove_dir_all`, before mutating the in-memory metadata; lock held | Final directory absent; on-disk membership still present. |
| `missing_clone_confirmed` | Forced `NotFound` branch at the same seam | No deletion is claimed; explicit force accepted a missing owned clone. |
| `removal_committed` | Immediately after the helper's successful metadata persist, before lock release | Only that member/mapping/setup metadata is removed; earlier batch commits survive. |
| `description_committed` | After describe's successful metadata persist, before lock release | Latest description committed with unrelated fields retained; no claim of guidance update. |
| `guidance_snapshot` | Add: after acquiring lock and loading latest metadata, before `agentmd::update`; invocation-wide | The exact input projection is fixed; lock remains held. Other callers report their actual lock state. |
| `guidance_agents_committed` | Immediately after `agentmd::update` persists AGENTS.md | Managed block replaced and user section preserved; CLAUDE.md/skills may still be old or absent. |
| `guidance_complete` | After successful `agentmd::update`, before language integrations; invocation-wide | AGENTS.md plus that helper's auxiliary updates completed. This is not a claim that all language integrations succeeded. |
| `mirror_prepared` | After successful mirror clone and initial fetch in host add/registration, before registry save | Valid fetched mirror exists; registration may still be absent. This is not an atomic mirror-creation event. |
| `registry_committed` | Immediately after successful config persist, within config lock | Registry entry committed; workspace state is unchanged by registration itself. |
| `refresh_selected` | After transport planning/identity validation, before selected fetch; no metadata lock | Chosen direct/mirror transport is fixed; no alternate retry is authorized. |
| `refresh_complete` | After selected refresh/propagation returns successfully | Actual Git refs may have changed; removal has not yet occurred. |

Do not put `membership_committed` inside the `with_metadata` mutation closure:
the closure only edits memory. Add a narrowly scoped internal completion seam in
`with_metadata` after save and before releasing its lock, or give the metadata
writer an explicitly scoped test operation tag. Use the same mechanism for
removal and describe. Tags must be carried by an RAII test context limited to
the current operation; unrelated fixture/config/metadata writes do not emit a
misclassified event. An operation context is volatile test state and does not
participate in product recovery.

Low-level metadata persistence also exposes `metadata_temp_written` after
`write_all` but before `persist`, and `metadata_replace_pending` immediately
before `persist`, under the calling operation tag. A kill there preserves the
previous manifest and may leave an unreferenced tempfile. Never assert that
temporary filenames or staging directories disappear on abrupt death. Completed
low-level persist maps to the appropriate semantic committed event, once.

Mirror internals and installed guidance files have additional writes. This
catalog does not expose every possible filesystem interruption. Later points
must name a real completed primitive and add a dedicated recovery assertion;
they must not label an entire multi-write helper atomic.

## 6. Deterministic returned-error injection

Keep returned-error injection distinct from process death. Provide a tiny
allowlist of `Fail` replies at explicit pre-operation points:

| Injection point | Skipped operation and resulting path |
| --- | --- |
| `clone_publish_pending` | Return an ordinary I/O error before non-replacing rename; exercise staged-clone cleanup, preserve occupied/final paths. |
| `metadata_replace_pending` | Return an error before `persist`; exercise published-orphan and deleted-clone accounting without pretending a commit happened. |
| `clone_delete_pending` | Return an error before `remove_dir_all`; preserve membership and clone bytes. |
| `guidance_replace_pending` | Return an error before AGENTS.md replacement; membership remains committed and CLI reports the actual guidance outcome. |
| `refresh_selected` | Return a transport error before the selected fetch; exercise fail-closed removal and prohibit alternate transport. |

Use fixed error kinds/messages such as `PermissionDenied` or a clearly marked
synthetic I/O error, not platform-specific integer errno values. Keep injected
errors at the same ordinary error-return boundary as real failures. No hook can
return success, authorize force, suppress validation, choose an arbitrary path,
run a command, rewrite content, or inject an error after a success event while
claiming the operation did not happen.

A pre-delete synthetic error does not cover partial recursive deletion. Initially
model that case with an explicitly attributed environment step: pause before
deletion, have the parent remove one known fixture-owned file, then inject a
deletion error. This verifies handling of the resulting damaged clone and must
be labeled as such. Separate OS tests must establish real deletion-failure
behavior; true deterministic crashes inside `remove_dir_all` need a later seam
and must not replace the production algorithm solely for testing convenience.

## 7. Integration harness and independent observations

Place the controller and fixture support under `crates/wsp/tests/support/`, and
table-driven scenarios in `crates/wsp/tests/workspace_crash.rs`. Keep protocol
types narrowly shared under the test feature so encoding does not drift; expected
filesystem projections must remain independent of product allocation/planning
helpers. All product actions use the binary, including retry and inspection.

Each case creates fresh mounted/sibling roots with the same metadata name,
isolated and host stores, and controlled Git remotes. Reuse the real-binary
fixture patterns in `tests/workspace_local.rs`, but independently parse output
and metadata for expected state. Use synthetic URL identities and controlled
Git URL rewriting or a local transport fixture; do not teach product validation
to accept invalid URLs or store fixture paths as canonical repo identities.
Clear inherited Git configuration and shell-wrapper variables, set fixture
identity/signing settings, disable interactive credential prompts, and keep all
setup commands absent unless a test explicitly owns and observes them.

For each planned barrier, the harness waits for the exact event, verifies the
expected filesystem projection while the child is blocked, and chooses resume,
allowed failure, or kill. Recovery is a new process/session with no dead-process
state, normally no barrier configuration, and optionally the opposite context.
Use semantic assertions on membership/mapping, literal origin bytes, checkout,
refs, worktree/index content, relevant Git config, registry, mirror inventory,
user guidance sections, and sibling root. Normalize only explicitly declared
volatile fields; retain unexpected files and differences as evidence.

Crashing commands need not produce final JSON. Non-killed commands must have
correct exit status, valid structured output, and truthful stages alongside
filesystem assertions. A hook event never substitutes for those observations.
Capture lock-file existence/PID separately from authority state: a mutator may
leave it behind, whereas read-purity tests still forbid creating it.

Concurrency schedules use `clone_staged` or `remove_preflight_complete` to pause
outside the lock while another invocation commits. A `clone_published` pause is
inside the lock: a peer cannot be asked to complete metadata work until the
holder resumes or dies. Test lock exclusion with an acknowledged pre-acquisition
event plus final commit ordering, not an arbitrary sleep or "no output for 50ms"
assertion. Test release after death by observing a second real mutation complete
against the same unchanged lock file after the parent has reaped the first.

Permissions/ACL confinement is a separate fixture capability. Absence of global
paths alone tests fallback, not denied access. A claimed denied-store profile
must prove the child identity cannot read/write sentinels, allow its explicit
control channel, and inventory mount/global/sibling effects separately. Host and
local concurrent effects require per-process scheduling/attribution rather than
comparing all global bytes across an overlapping local invocation.

## 8. Required scenarios and adequacy

| Scenario | Required evidence |
| --- | --- |
| Kill at `stage_created` and `clone_staged` | Membership/final slot unchanged; leaked staging is tolerated; ordinary retry succeeds without adopting staging as membership. |
| Kill at `clone_published`, retry local and host | Clone exists without membership; dirty/user-edited compatible clone is adopted intact; no registry, setup, discovery, or defaults replay on adoption. |
| Kill at `adoption_validated` | User-created clone remains unchanged and unrecorded; ordinary retry adopts using disk facts only. |
| Kill at `membership_committed` | Retry reports existing member, preserves changed developer checkout/config, repairs add guidance, and does not replay host extras. |
| Metadata failure before persist | Old manifest remains parseable; published clone stays available for adoption; no compensating deletion of final path. |
| Two same-identity additions | Both reach staging, first commits, second preserves the winner and discards only its own stage. |
| Colliding basenames and concurrent describe | First mapping is stable; later add rereads metadata, preserves peer member/description, and chooses the independently expected free candidate. |
| Foreign destination inserted before publication | Existing foreign content is neither overwritten nor removed; exclusive publish or validation fails visibly. |
| Kill at `clone_deleted` | Membership remains with missing clone; normal retry fails nondestructively; explicitly forced retry clears owned missing metadata. |
| Kill at `removal_committed` | Removed member stays removed, surviving mappings and registry remain; repeated removed identity may return membership error. |
| Batch deletion/add failure | Earlier commits survive a later fault; only actual successful members appear in results and metadata. |
| Kill after AGENTS.md persist | Partial auxiliary guidance is represented honestly; add retry converges, preserving exact user content. |
| Guidance returned error | Membership remains; add reports required-stage failure instead of overall success. Other commands retain their current contract until separately changed. |
| Mirror prepared, registry absent | Workspace unchanged; retry behavior is observed per concrete command. Explicit registration currently rejects an existing orphan mirror, so do not assert universal registration convergence. |
| Registry committed, process killed | Registration persists while workspace bytes remain unchanged; existing-member host retry still preserves the clone. |
| Selected mirror/direct refresh failure | No deletion, membership retained; mirror failure produces no direct fallback in that invocation. |
| Same-name roots | Every workspace barrier refers to mounted root; sibling remains unchanged through crash and opposite-context retry. |
| Protocol and release isolation | Wrong token/PID/session, unknown point, skipped occurrence, EOF, timeout, oversized message, duplicate response, and early child exit fail the harness; ordinary release ignores all control variables. |

Each test must witness its selected event before it can pass. Test both resume
and kill at important points so a hook that simply prevents all progress cannot
pass. Include one equivalent uninterrupted run to detect instrumentation changing
normal behavior. Run the essential publish/adopt, removal, kill/reap, and lock
release cases on Linux, macOS, and Windows; compile-only cross checks do not
establish Windows rename/termination behavior.

Follow the repository's testing-and-review guidance during implementation.
Demonstrate that key assertions fail when their guard is deliberately broken:
move publication acknowledgment before rename, suppress membership persistence,
allow overwrite, replay setup on adoption, or erase a peer description. Keep such
mutations ephemeral and outside production fault APIs. The existing Quint
mutations test model properties; they do not replace these binary-test checks.

## 9. Trace integration and rollout

1. Add the gated control protocol, catalog, build exclusions, and controller
   tests. Verify an uninstrumented binary has no behavior change.
2. Add staging/publication/adoption/membership barriers and real-binary crash
   recovery scenarios. These are the first usable crash witnesses.
3. Add per-member removal, metadata failure, guidance, description, and
   host-infrastructure points. Keep product/model discrepancies visible as
   separate issues and unsupported expectations, not broadened hook behavior.
4. Add deterministic concurrency schedules and the three-platform CI matrix.
5. Only then implement ITF normalization/replay in Rust test support and `xtask`.
   Store source/model revision, seed, profile, catalog version, raw trace,
   normalized trace, barrier transcript, and failing-step details with failures.

The existing normalized schema's barrier contains only `id` and `point` and
disallows extra fields. For multi-repo/occurrence selection either restrict v1
replay to an unambiguous single-repo invocation or introduce a versioned schema
with repository/occurrence selectors and explicit resume/schedule semantics.
Do not silently add fields to v1 or infer a batch target from whichever event
arrives first. Validate the whole trace and barrier support before invocation.

Define a reviewed mapping from model actions to invocation/barrier/resume/kill
groups. `localPublish` and `localCommit` are phases of one command, not separate
CLI invocations. Repeated `Crash` actions, impossible orderings, unsupported
model states, and ambiguous mappings are rejected as not replayable. Reaching
the end of a supported trace requires every expected event and independent
postcondition, not merely a successful subprocess exit.

Completion for the first implementation means reproducible publish/crash/adopt
witnesses in both contexts, kill-driven lock release on all supported OSes,
negative release-build checks, clear catalog coverage, and zero product runtime
interface changes. It does not mean the bounded model proves the implementation.

## 10. Risks and explicit limits

- Hook drift can silently change the meaning of a boundary. Keep hooks adjacent
  to the primitive, document lock state, verify both sides independently, and
  version incompatible catalog changes.
- Pausing while holding a lock perturbs scheduling. It is useful for a specified
  interleaving, not a performance test or proof against arbitrary external Git
  writes between safety checks and deletion.
- Recursive deletion, Git internals, auxiliary guidance writes, mirror creation,
  and power loss have finer failure states than the first catalog exposes.
  Unsupported interruption windows remain explicit coverage gaps.
- Loopback restrictions and descendant cleanup vary by platform. Do not skip a
  required platform silently or substitute permission simulation for enforcement.
- A feature is not inherently test-only. Dual opt-in, profile guards, separate
  targets, behavioral exclusion tests, and release artifact selection are all
  required parts of this design.
- Tests must not manufacture recovery provenance. Leaked tempdirs, lock files,
  and the control transcript are observations; product retry cannot consult them
  to decide whether a final clone is safe to adopt or delete.

If implementation reveals a product mismatch or tooling friction, report the
concrete reproducer and suggest a focused issue in the relevant repository.
Do not silently change the model, CLI contract, or safety checks to obtain a
green crash suite.
