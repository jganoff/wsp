# Workspace-local operation in isolated sandboxes

**Status:** Implementation in progress. This document retains the original
design rationale, scope boundaries, and acceptance gates.
**Date:** 2026-09-18
**Author:** Astra, at the maintainer's request
**Issue:** [#168](https://github.com/jganoff/wsp/issues/168)
**Related:** #70 (complete JSON coverage), #170 (persisted-state compatibility)

## Goal and scope

An agent starting in an existing writable workspace must be able to discover
its repositories, inspect their Git state, change the workspace description,
add a repository by URL, fetch, and sync with no accessible host wsp state.
The workspace's actual mounted location is authoritative. No HOME, registry,
mirror, template, or approval store is required for that supported subset.

The runtime still needs the installed wsp binary, Git, their runtime libraries,
and OS facilities. Network commands need network access and whatever Git
credentials the caller explicitly provides. "Only the workspace" means only
that project/data boundary is available; it does not imply fetching works
without a reachable remote, or that wsp supplies a sandbox or credentials.
Temporary files created by this mode must fit inside the writable workspace.

This is workspace portability, not an agent scheduler, task database, MCP
server, Git wrapper expansion, baseline-copy feature, or universal JSON audit.
Workspace creation/removal/rename/recovery, global administration, and setup
approval remain outside the new portable guarantee. `repo rm` and `exec` are
workspace-local commands and are included: callers retain responsibility for
explicitly choosing destructive removal or arbitrary command execution.
Existing confirmation and `--yes`/`--force` contracts stay in force. Do not
solve this by creating a workspace-local imitation of global state,
automatically registering repositories, or modifying clone remotes.

### Returning from a sandbox to a host

Workspace-local changes are ordinary durable workspace state. A repository
added while isolated is a member because it is recorded in that workspace's
`.wsp.yaml` and exists as a normal clone with its own `origin`; it is not a
temporary sandbox overlay. When the same workspace later has global wsp access,
all supported workspace commands must continue to see and operate on it.

Returning to a host must not silently register the repository, create a
mirror, rewrite its `origin`, move its directory, or normalize away its
workspace-local directory mapping. A host user may explicitly run `wsp
registry add <url>` later to opt into shared-mirror infrastructure. Until then,
transport planning uses the clone's matching `origin` directly and reports why;
the absence of a registry entry is supported workspace-local membership, not a
broken workspace. A usable matching mirror can be selected only when it already
exists and doing so does not change clone or registry ownership.

`doctor` distinguishes this supported condition from a malformed or mismatched
clone: an unregistered member whose actual `origin` matches its metadata
identity is informational and may explain that it is using direct transport.
It remains an error when the clone is missing, its remote identifies a different
repository, its Git storage escapes the permitted workspace, or metadata is
malformed. `doctor --fix` must never turn a workspace-local member into a
global registration as an incidental repair.

**Membership-first idempotency is required in every environment.** Before a
host or local `wsp repo add` performs registry registration, mirror creation,
Git-default application, template discovery, setup resolution, or clone work,
it checks whether each requested identity is already a compatible workspace
member. Repeating a sandbox add after returning to the host reports
`already_present`; it is not deferred global registration. It must not create
a mirror, modify clone remotes/configuration/branches, replay setup or
discovery, or rewrite generated files except to repair a separately reported
stale derived file from current metadata. A mixed request handles each identity
independently: only genuinely new host additions follow the host registration
flow. This also governs retries after interruption, regardless of which
environment performs the retry.

## Source findings and first slice

Read these before implementing: `AGENTS.md`, `docs/design-tenets.md`, the body
and design discussion of #168, and
`docs/reference/plans/2026-05-10-wsp-sync-conflict-resolution.md`.
The longer issue comment is context, not authorization to implement its agent
planning, adapter, or baseline features in this change.

The working tree already contains an uncommitted fix in
`crates/wsp/src/cli/describe.rs` and a regression fixture in
`crates/wsp/tests/workspace_local.rs`. Preserve them. The fix keeps the detected
workspace directory; the fixture sets HOME/XDG paths and tests an empty
workspace. This is a useful first slice, but it does not establish filesystem
confinement, real-clone behavior, inaccessible HOME, or startup independence.

Current dependencies that must be removed or made explicit:

| Location | Current constraint |
| --- | --- |
| `main.rs`, `config.rs::Paths::resolve` | Global paths and config are resolved before ordinary command dispatch. |
| `config.rs::Config::load_from` | `Path::exists()` treats some access failures as absence. |
| `main.rs`, `hints.rs` | Successful non-JSON commands can write version/hint state globally. |
| `cli/status.rs` | Reloads config, loads global ignore rules, and optionally invokes GitHub PR lookup. |
| `cli/fetch.rs` | Chooses mirror paths; some config failures become defaults. |
| `cli/sync.rs::fetch_workspace_mirrors` | Refreshes mirrors before clone propagation and sync. |
| `cli/add.rs` | Registers URLs, creates mirrors, imports templates, and invokes setup handling. |
| `workspace.rs::add_repos` | Clones into final paths, removes them on failure, and renames existing clones on basename collisions. |
| `filelock.rs::read_config/read_metadata` | These are exclusive locking operations that create/write lock files, despite their names. |
| `cli/doctor.rs` | Global checks and repairs precede workspace checks. |

## Ownership and capability model

### Authorities

1. `.wsp.yaml` owns workspace identity, membership, directories, branch intent,
   and workspace settings.
2. Each ordinary clone owns its actual remote URLs, refs, checkout, and Git
   operations in progress. A registry URL is never a substitute for its remote.
3. Global config owns registered URLs and machine defaults. Global stores own
   mirrors, templates, and setup approvals.
4. The caller owns filesystem confinement, network access, credentials, and
   permission to execute arbitrary subprocesses.

Document the direct transport exception in `docs/design-tenets.md` before
shipping it. Available host infrastructure stays mirror-backed. Direct access
is permitted only for supported operations when the necessary infrastructure
is absent, inaccessible, or cannot be written. Network failure, malformed
state, and remote mismatch are not fallback triggers.

### Invocation context

Add `crates/wsp/src/context.rs` for command policy and invocation resolution;
put reusable global-state reading and capability types in
`crates/wsp-core/src/config.rs` or a narrowly scoped `capabilities.rs`.
Do not make every local function accept a fake `Paths` value.

The context should contain:

- Actual invocation directory from `shellcd::invocation_dir()`, optional
  detected workspace root and parsed metadata, and command scope/requirements.
- A typed global-state result and optional validated config/path information.
- Separate capabilities for registry read/write, mirror read/write, templates,
  and approval read/write. One writable-config boolean is insufficient.
- A resolved workspace-local versus host policy and structured reasons.
  Per-repository transport is selected separately, so mixed availability is
  representable.

Discovery happens before requiring global path resolution. Distinguish "not a
workspace" from unreadable, malformed, or unsupported workspace metadata.
Do not scan past an invalid enclosing workspace and act on a different one.
Keep help/version and graceful completion independent of usable global state.

For commands accepting a workspace name, an omitted name uses the detected
root. An explicit name equal to the detected metadata name may use that root
as well, even when its basename changed on mounting. An explicit different
name requires normal global resolution; never silently substitute CWD.
Preserve the existing freeform positional interpretation in `describe`.

### Global states and decisions

| State | Detection and behavior |
| --- | --- |
| Absent | Missing relevant path or no resolvable HOME/data path. Supported local operations use workspace/clone authorities and built-in defaults. Do not create global directories. |
| Unreadable/unavailable | Access denial or confinement prevents reading the relevant store. Supported local operations proceed with a reason; do not report an empty registry as fact. |
| Read-only | Valid readable state may supply defaults and unambiguous URL resolution; writes, lock-file creation, imports, and approval updates are forbidden. Direct transport is selected where mirror writes are unavailable. |
| Available | Valid readable state and required write capabilities permit existing host behavior. |
| Malformed | Parsing, semantic validation, or invalid store contents fail. Commands needing that state error before mutation; global config malformed during invocation resolution is an error for supported workspace calls too. `doctor` reports corruption as a failed check and can still inspect local state. Never replace corruption with defaults. |
| Relocated | Not a global availability state. The detected root remains authoritative regardless of configured `workspaces_dir`; optional global stores are classified independently. |

Implement explicit `std::io::ErrorKind` handling rather than `exists()` or
`unwrap_or_default()` at these boundaries. Preserve unsupported-format/version
information; do not guess that an unknown persisted authority is empty.

Capability discovery must not create probes, lock files, hint files, mirrors,
or directories. For read commands, even temporary writes violate the contract.
Use non-mutating platform access/mount/ACL information where required; a Unix
mode-bit check or Windows read-only attribute alone is not adequate. Keep a
capability "unknown" internally when the OS cannot establish it. Permission
checks race: the actual operation remains authoritative, and a later failure
must be reported without claiming rollback. An unknown capability must never
justify creating a global store during a local invocation. Settle and test the
platform implementation in phase 1 before depending on it in transport code.

Read valid config once per invocation. Built-in defaults plus available global
defaults plus workspace settings plus explicit flags retain existing
precedence. Unavailable settings are not synthesized from clone files. Do not
copy defaults or approvals into metadata to make subsequent invocations work.

## Command capability matrix

Publish this matrix through existing help/generated-skill mechanisms, without
adding a discovery command. "Local" below means workspace-only policy.

### Portable command contract

The following is the release contract for an agent whose process can read and
write the mounted workspace, its ordinary clone directories, and the `wsp` and
Git executables, but cannot rely on any other wsp-managed path. Every supported
workspace command is invoked from the workspace root or one of its clone
subdirectories. The omitted workspace positional therefore always means the
detected mounted workspace.

| Invocation | Guarantee in workspace-local mode | Notes |
| --- | --- | --- |
| `wsp`, `wsp st`, `wsp status` | Supported | Read metadata and local Git state only. Bare `wsp` retains its status dispatch. No PR lookup, global ignore read, lock, hint, version record, index refresh, or network access. |
| `wsp diff [-- <git-diff-args>]` | Supported | Runs only in member clones. Git arguments retain their existing meaning. |
| `wsp log [-- <git-log-args>]` | Supported | Runs only in member clones and does not fetch. |
| `wsp repo ls` | Supported | Lists metadata membership and paths from the mounted workspace. |
| `wsp describe <text>` and `wsp describe -- <text>...` | Supported | Updates the detected mounted `.wsp.yaml` and its workspace-local generated guidance only. |
| `wsp repo add <url>...` | Supported | Every new repository needs an explicit URL. It stages and publishes only inside the mounted workspace; it never registers globally, creates a mirror, imports templates, or runs setup. The member remains usable when the workspace later returns to a host. |
| `wsp repo add <shortname>` | Supported only with an accessible, valid, unambiguous registry entry | Otherwise fails before mutation and tells the caller to pass a URL. |
| `wsp repo add --no-fetch <url>` | Supported | Preserves the existing flag meaning: skip a mirror refresh. A direct clone may still contact its URL. |
| `wsp repo rm [--force] <repo>...` | Supported | Resolves names from mounted metadata, performs the existing removal safety checks using mirror or direct-`origin` refresh transport, then deletes only selected member clones and updates local metadata/guidance. `--force` keeps its existing meaning. |
| `wsp fetch [--prune]` | Supported | Fetches only the detected workspace's existing clones. Uses a matching available mirror when possible; otherwise fetches each clone's `origin` directly and reports the transport. |
| `wsp sync`, `wsp sync --dry-run`, `wsp sync --abort --yes` | Supported | Operates only on the detected workspace's clones. Direct transport is selected only for unavailable mirror infrastructure; conflict, dirty-tree, confirmation, and exit-code behavior remain unchanged. |
| `wsp exec -- <command>...` | Supported | Runs the caller's explicit command in each mounted member clone. It does not gain sandboxing or approval semantics; the caller's sandbox and command policy remain authoritative. |
| `wsp doctor` | Supported as diagnostics | Inspects the mounted workspace and reports unavailable global checks. It performs no repair and no global enumeration. |
| `wsp help`, `wsp --help`, `wsp --version`, `wsp completion <shell>` | Supported | Must remain usable without HOME or global config. Completion generation is best effort and does not initialize state. |

An explicit workspace name is only portable when it names the workspace already
detected from CWD, including when the mounted directory basename differs from
`.wsp.yaml`'s `name`. An explicit different workspace name requires host global
workspace authority and must never be redirected to the mounted workspace.

These commands are deliberately **not** in the workspace-local guarantee:

| Invocation | Required behavior without host authority |
| --- | --- |
| `wsp ls`, `wsp cd`, `wsp new`, `wsp init`, `wsp rename`, `wsp rm`, `wsp recover` | Fail before mutation or enumeration with a host-authority recovery message. |
| `wsp registry ...`, `wsp template ...`, `wsp config ...` | Fail before mutation. They retain their existing host/global authority contracts. |
| `wsp fetch --all`, setup-command operations | Do not receive a portable guarantee. `fetch --all` requires global authority; setup execution remains outside this operation contract. |
| `wsp doctor --fix` | Fail before repair and explain that fixes require a normal host invocation. |

| Command/scope | Local support | Required effects and restrictions |
| --- | --- | --- |
| `help --json`, version, completion | Yes | No global requirement; completion remains best effort. |
| Bare `wsp`, `st`, `diff`, `log`, `repo ls` | Yes | Metadata and clone reads only; no locks, hints, version writes, optional PR network lookup, or global repair in local mode. |
| Workspace detection; existing guidance/skills | Yes | Read the mounted files directly; do not resolve through host workspace storage. |
| `describe` | Yes | Metadata lock/write and generated guidance in this workspace only. |
| `repo add <url>` | Yes | Stage/publish a clone, update membership and generated local files; no local-mode registry/mirror/import/approval writes. |
| `repo add <shortname>` | Conditional | An accessible valid registry must resolve one explicit URL unambiguously; otherwise request a URL. |
| `repo add --template` | No new local guarantee | Reject before mutation under local policy with guidance to pass repository URLs. |
| `repo add --no-fetch` | Yes | Preserve its actual meaning, "skip fetching mirrors before cloning." Local direct clone still contacts the remote; document that this flag is not an offline guarantee. A validated existing clone can be adopted without fetching. |
| `fetch` for current workspace | Yes | Fetch selected clones directly when mirrors unavailable; preserve `--prune`; never visit another workspace. |
| `sync [current-workspace]` | Yes | Same transport choice, then existing guard/resume/rebase/merge semantics. |
| `sync --dry-run` | Yes | Local preview only, no capability write probes, fetch, continuation, or file generation. |
| `sync --abort` | Yes | Clone-local Git abort with existing destructive confirmation behavior. No mirror requirement. |
| `doctor` in local workspace | Yes | Inspect current workspace, report unavailable global checks explicitly; no global tree enumeration or repair. |
| `doctor --fix` | No new local guarantee | Reject before any repair in local policy; explain which fixes require a normal host. A separately scoped local-repair feature can follow. |
| `fetch --all`, registry/template management | No | Require their real global authority; do not reinterpret as current-workspace commands. |
| `repo rm` | Yes | Resolve names only from workspace metadata; use the selected per-clone transport for removal safety; delete member clones and update local derived files without changing mirrors or registry state. |
| `new`, workspace `rm`, `rename`, `recover` | No | Existing host authority and safety contracts; fail before mutation if unavailable. |
| `exec` | Yes | Executes only the caller-supplied command in detected member clones. It remains effectful and is sandboxed by the caller, not wsp. |
| Setup/approval commands and other commands | Unchanged | Do not grant implicit setup approval. Audit unsupported operations so they fail before mutation. |

Host administration must still be able to intentionally initialize absent
global state through its existing commands. The local fallback policy does
not turn `init` into a no-op or globally prohibit registry creation.

## Fetch and sync design

Create a shared transport planner, preferably
`crates/wsp-core/src/transport.rs`, used by `cli/fetch.rs`, `cli/sync.rs`, and
the refresh portion of `cli/remove.rs`. Represent a plan as `Mirror { path }`
or `Direct { remote: "origin", reason }`, with explicit per-repository planning
failures. Do not duplicate fallback logic in the command handlers.

For each repository:

1. Resolve the clone from metadata under the actual workspace root. Validate
   membership/path information and the ordinary clone's Git dependencies.
   Reject missing clones, escaping symlinks/reparse points, linked Git storage
   outside the permitted workspace, and inaccessible object alternates with
   actionable diagnostics. Do not convert a linked worktree into a clone.
2. Ask Git for the actual `origin` fetch URL. Validate its repository identity
   against metadata. Different URL spelling/protocol for the same identity is
   acceptable; a different repository identity is an error, never a reason to
   rewrite the remote. Missing or ambiguous fetch URLs fail that repository.
   Preserve fork/upstream semantics already represented by existing commands;
   do not invent a new remote-selection option in this issue.
3. Select mirrors when their required infrastructure is available and their
   upstream identity matches. If absent/inaccessible/read-only, select direct
   origin transport with a precise reason. Also select direct transport for a
   valid workspace-local member that has no matching registered mirror after
   global access returns: registry absence is not permission to register it or
   a reason to stop supporting the workspace. A usable existing matching mirror
   may still be used. A corrupt mirror is an error, not an absent cache.
4. Freeze the decision before network I/O. A mirror authentication, DNS,
   timeout, ref-lock, corruption, or other refresh failure is a reported
   failure. Do not retry via direct origin after attempting mirror fetch.
   If permissions change after planning, report failure and let the next
   invocation plan from the new state.
5. Direct fetch addresses `origin` explicitly using argument arrays, preserving
   clone remote/refspec configuration and prune behavior. Validate that the
   refspec cannot update local branch refs or another remote's namespace; fail
   clearly on unsupported custom mappings rather than rewriting them.
   Do not use a bare `git fetch` that selects a branch-dependent remote.
6. Mirror propagation targets only selected clones in this workspace. Record
   propagation failure as a repository failure, not successful fetch followed
   by an invisible warning. Direct transport has no propagation step.
7. `repo rm` consumes the per-repository refresh result before its existing
   branch/removal safety classification. A refresh failure remains a safety
   failure and blocks deletion unless the existing `--force` contract explicitly
   permits proceeding. It never mutates the registry or mirror store. Name
   resolution uses workspace metadata alone, so removing an unregistered
   sandbox-added repository works.
8. `sync` consumes the per-repository refresh result and then retains its
   existing in-progress operation detection, dirty/wrong-branch guards,
   strategy, conflict retention, and exit precedence (0 success, 1 failed,
   2 paused). A failed refresh must not start or continue sync for that repo;
   healthy peers still proceed. Dry-run and abort bypass transport execution.

Audit discovery/template import transitively through propagation helpers;
suppress global imports in local mode and report the skipped outcome. Do not
silently enable them just because a template directory is readable. Keep
network/progress output on stderr and stable repository ordering in JSON.

## Safe local repository addition

### Rules

Keep host add behavior mirror-backed. Introduce a local path in `cli/add.rs`
and core helpers in `workspace.rs` or `workspace_add.rs`; do not reuse the
current clone-directly-to-final-directory and cleanup branch.
Refactor both paths around membership-first idempotency: resolve the workspace
membership snapshot before any registration or other global side effect, then
process only genuinely new identities. This corrects current host behavior as
well as the local path; preserving the current ordering would turn a harmless
cross-context retry into unexpected registration, discovery, or setup.
Reuse safe Git/branch initialization logic after separating it from mirror
creation. Validate all request URLs, branch overrides, and names before any
clone. Explicit URLs take precedence over a registry's transport spelling;
never silently replace the user's URL with a registered one.

Existing repository directory names are fixed during local add. Select an
unoccupied deterministic directory for each new identity: existing basename
policy first, then existing host/owner disambiguation conventions, checking
case-insensitive collisions where appropriate. Record non-default names in
the existing `Metadata.dirs` map. Never rename an existing clone to make a new
one fit. Check the semantics of `check_missing_dirs_map`/`check_stale_dirs_map`
and every `compute_dir_names` consumer so a valid asymmetric mapping is not
later "repaired" by moving or forgetting it. The same stable-mapping invariant
applies to host add, `repo rm`, and `doctor`: none may recompute and rename
surviving clones merely because a workspace-local member exists.

This uses existing metadata fields, but extends the documented layout
invariant. Coordinate with #170: old supported versions must not destructively
normalize this layout. If that cannot be guaranteed, version/gate the change
before release; do not hide it as an internal refactor.

### Three-phase operation, one repository at a time

1. **Snapshot under the workspace metadata lock.** Read current membership,
   branch, settings, and mappings; resolve input identity and proposed final
   path. Validate root containment and existing directories using
   `symlink_metadata`, not `exists()`. If already a member, validate its clone
   and report `already_present`; still refresh stale generated guidance later.
   Do not hold this lock during cloning or setup.
2. **Clone into owned staging.** Create a uniquely named private temporary
   directory under the workspace, e.g. `.wsp-add-<random>/clone`, using
   `tempfile` exclusive creation. This keeps publication on one filesystem and
   avoids inaccessible system temporary directories. Stage only new clones;
   never clone into the final directory. Initialize the requested tracking or
   fresh workspace branch using the current semantics, including explicit
   per-repository branch overrides and empty/default-branch cases. Validate
   the staged origin, refs, branch, and lack of external object dependency.
   No setup, template import, or user hook supplied by workspace metadata runs
   in this phase. Clean up only the staging directory owned by this live
   invocation, and only when its ownership remains established.
3. **Recheck and publish under the metadata lock.** Reload metadata. Recheck
   membership, branch intent, directory mappings, and final path. If a peer
   already added the same identity compatibly, report `already_present` and
   clean only this invocation's staging. Conflicting intent fails without
   changing either clone. Recompute only the new repository's available name
   if another addition occupied the earlier proposal. Publish the completed
   clone with an atomic **no-replacement** directory rename, then save updated
   metadata atomically while retaining the same lock.

`std::fs::rename` after `exists()` is not a portable no-replacement operation:
on Unix it can replace an empty directory created in the intervening race.
Implement a tested safe abstraction for Linux/macOS/Windows using a maintained
safe dependency around each platform's exclusive rename primitive; the crate
must retain `deny(unsafe_code)`. Verify exact supported targets and filesystem
semantics in the first implementation spike. Never substitute check-then-rename
or a copy that merges into a pre-existing destination. If a supported platform
cannot provide the primitive, stop that slice and resolve the design before
claiming safe local add there.

Publication and manifest persistence are two filesystem operations, not a
transaction. Do not promise atomicity across them. If publishing succeeds but
metadata save fails, leave the complete final clone intact and return separate
clone/metadata outcomes. Do not rename it back or delete it. Retry can adopt a
complete existing ordinary clone only after confirming identity, expected
branch, path ownership/containment, and compatible request. Adoption changes
membership only: preserve dirty work, refs, config, origin, and checkout. A
pre-existing directory that does not pass these checks is a collision, even
if it is empty. No implicit replacement, cleanup, remote rewrite, or branch
switch is allowed. This inspection also handles an interruption between
publication and manifest persistence without a journal.

An interrupted staging clone is not a member and cannot block a retry's new
unique staging path. Preserve abandoned staging directories; do not guess
that a PID or age makes recursive removal safe. `doctor` reports them with
their exact paths and manual inspect/remove guidance. Root-content checks
must recognize this managed temporary namespace without hiding arbitrary
content that merely resembles it. There is no new cleanup daemon, transaction
journal, or recovery command. Bounded automatic orphan cleanup is deferred.

### Derived files and setup

After each membership change (and on an idempotent retry), reload current
metadata under the same workspace serialization discipline and update
generated guidance/local language integration files from that latest state.
Serialize generation against concurrent metadata writers so an older snapshot
cannot overwrite newer guidance. Keep this local phase short; no network,
prompts, approval lookups that write, or setup under the metadata lock.
Make managed document replacement atomic, preserve user sections, and report
each generation failure. A failure after membership commit is partial success,
never a reason to delete the clone.

Do not call `template::auto_register`, `discovery::prompt_and_import`, or
global-writing hint/setup helpers in the local path. Resolve setup declarations
only to explain their disposition. Default local behavior is to skip setup
without requesting or persisting approval; report whether there were no
commands, approval was unavailable, approval was absent, or execution was
deferred by local policy. Readable approvals do not imply permission to write
approval bookkeeping or to broaden this portable operation to arbitrary
external effects. An explicitly invoked existing setup command retains its
own contract. Never replay setup automatically after an interrupted add.

## Structured output contract

Use existing `Output` variants/rendering and additive fields. Keep current
top-level fields and exit behavior unless the extension requires a documented
partial-failure result. In particular, preserve `repo add`'s existing mutation
message while adding operation details; do not wrap all commands in a new
envelope or silently change an existing array into an object.

Add shared serializable types in `wsp-core/src/output.rs` for:

- Invocation context: actual workspace path, `mode: host|workspace_local`,
  relevant global-state classification/reasons. Do not expose credentials.
- Per-repository transport: `mirror|direct|none` and fallback reason, present
  on fetch/sync results and add results where applicable. Allow mixed results.
- Add stages: clone (`created|adopted|already_present|failed|not_attempted`),
  membership (`updated|unchanged|failed|not_attempted`), generated guidance
  (`updated|unchanged|failed|not_attempted`), setup (`not_configured|skipped|ran|failed`)
  and reason/error for each, plus identity and actual destination.
- Template import outcome (`skipped` plus reason in local mode) and local
  integration failures, with explicit per-stage detail rather than stderr-only
  warnings. Use existing host setup semantics for host-mode results.

For add, clone/membership/guidance failure yields nonzero exit with the complete
per-repository result, even if earlier repositories succeeded. A documented
policy skip (setup/import) is not a clone failure and does not alone make the
command fail. Preserve sync's paused precedence. Preflight errors still use
the existing `ErrorOutput.error`; add optional stable code/recovery fields if
needed, without replacing that field. Never classify errors by their English
message. A killed process may produce no final JSON; callers inspect membership
and clones before retrying rather than inferring success from silence.

Every new visible field needs a populated `sample()`. Update the custom sync
serializer, CLI output renderer/exit calculation, generated contract source,
and `tests/agent_contract.rs` expectations as needed, then run `just skill`.
Tests must assert valid stdout JSON for success, preflight failure, mixed
results, and partial add; progress and diagnostics belong only on stderr.
Do not expand this issue into the unrelated remaining #70 command inventory.

## Diagnostics and generated agent instructions

`doctor` must distinguish unsupported capability from corrupt local data.
In local mode, inspect only the detected workspace and readable relevant
facts. Report unavailable global checks as deliberately unperformed, with
guidance to rerun on the host when global administration is required. Ordinary
absence of optional infrastructure is informational, not "workspace broken."
The same holds after returning to a host: a workspace-local member absent from
the registry is valid when its clone remote matches metadata and uses direct
transport. Corrupt metadata/global config, missing clones, inaccessible Git
storage, remote mismatch, incomplete published clones, and stale guidance need
specific failed/warning checks. Every warning/error needs useful next action.
Do not recommend `doctor --fix` as a repair available inside a local-only
sandbox or as a way to auto-register a workspace-local member.
This intentionally supersedes automatic unregistered-repository repair: current
metadata has no trustworthy provenance that can distinguish a sandbox-added
member from a historically unregistered valid member. Both remain supported;
registration is an explicit administrative operation. Coordinate the behavior
change with the completed #65 work and release notes.

Update `agentmd.rs::build_marked_section` and embedded/generated skills/help:

- Start with `wsp st --json` and `wsp repo ls --json` from the mounted workspace.
- Describe supported local mutations and their partial-result handling.
- Say agents edit repository files directly; authorized wsp operations may
  update managed root metadata/guidance. The current blanket "do not modify
  any root files" wording must not imply that `wsp describe` is forbidden.
- Explain direct transport, remote authority, skipped setup/imports, and host
  commands that are unavailable. Do not prescribe `registry add` or `doctor
  --fix` as a universal missing-global-state workaround.
- Existing installed instructions are readable without regeneration; invoking
  read commands must never rewrite them. Regenerate on supported mutations.
- State that copying a repository set into a new workspace does not reproduce
  its commit baselines, if that workflow is mentioned at all.

Update `docs/ARCHITECTURE.md` and appropriate existing help topics with ownership
and runtime policy. No new skill, command, flag, or external adapter is needed.

## Implementation sequence and concrete completion gates

Each phase should be independently reviewable. Do not advertise the complete
feature or close #168 after only the read/describe phases.

1. **Lock the contract and prove platform primitives.** Update the tenet
   exception, capability matrix, output design, and layout compatibility notes.
   Inspect #170's current requirements. Prototype non-mutating capability
   checks and exclusive directory publication with tests on all three OSes.
   Resolve safe dependency availability before building local add around it.
2. **Workspace-first invocation.** Add `context.rs`, split `Paths::resolve`
   responsibilities, and change `main.rs`/`cli/mod.rs::dispatch` to route local
   handlers without mandatory global `Paths`. Keep a host-path accessor that
   returns an explicit authority error. Remove lossy config fallback in
   migrated handlers. Keep unsupported host handlers behind capability checks.
   Gate hints/version/GC and completion accordingly. Test absent HOME, denied
   global paths, readable corrupt config, nested CWD, and relocated roots.
3. **Inspection, describe, and diagnostics.** Migrate `status.rs`, `diff.rs`,
   `log.rs`, `repo_list.rs`, `describe.rs`, and local doctor dispatch. Make
   global `.wspignore` optional while retaining built-in and workspace ignores.
   Preserve the existing describe patch and extend its real-binary fixture.
   Read operations must work with a read-only workspace and leave both local
   and global state unchanged. Add context output/samples for these commands.
4. **Direct transport and portable removal.** Implement `transport.rs` and
   explicit Git fetch/clone helpers in `git.rs`; migrate `fetch.rs`, sync and
   removal refresh planning, and mirror propagation result handling. Make
   `repo rm` resolve names from workspace metadata and update only the mounted
   workspace after existing safety checks. Verify normal-host mirror refresh,
   direct fallback reasons, remote mismatch, failed refresh, removal safety,
   and sync conflicts. No fallback after a selected network transport fails.
5. **Local add.** Split `cli/add.rs` host/local flows, stage new direct clones,
   introduce exclusive publication, stable layout selection, and adoption
   recovery. Extend output/rendering for stage results. Serialize derived-file
   generation using current metadata and suppress global side effects. Prove
   concurrent same/different-identity adds and every interruption boundary.
6. **Agent contract and release fixtures.** Update `agentmd.rs`, generated
   `skills/wsp-manage/SKILL.md`, help samples, architecture and user docs.
   Extend both `scripts/smoke.sh` and `scripts/smoke.ps1` for the supported local
   journey, including plain output. Add enforced-access fixtures to
   `.github/workflows/ci.yml` through Rust test/xtask infrastructure. Review
   the complete supported-command matrix on Linux/macOS/Windows.

Avoid incidental fixes to unrelated host add races in the same slice. Track
the observed direct-to-final cleanup/global-registration race separately if
it cannot be fixed by a shared narrowly scoped primitive without changing host
semantics. Never knowingly reuse that unsafe path for local add.

## Verification matrix

Fixtures use real Git clones, a workspace A, unrelated workspace B, and a
separate global tree. Snapshot/hash files and directory inventories in B and
global state before and after supported local operations. Include hint/version
files, locks, templates, approvals, mirrors, and nested stores in assertions.
Inspect A for shadow state; only documented metadata/derived/staging files and
clone effects are permitted. For read-only commands, snapshot A too.
Audit Git inspection subprocesses for index refresh and lazy object fetch:
use Git's supported controls such as `GIT_OPTIONAL_LOCKS=0` and disabling lazy
fetch where applicable, so a nominal read cannot rewrite the index or contact
a promisor remote. Report unavailable objects rather than silently hydrating
them during inspection.

Use synthetic repository identities such as `git@test.local:owner/repo.git`;
route test transport to controlled local bare repositories through Git's test
environment/config. Configure test commit identity and disable signing using
existing helpers. No public network or developer credentials are needed.
At least one enforced-isolation transport fixture must reach its remote
through a controlled endpoint rather than accidentally depending on an
out-of-sandbox file URL. Keep remote serving and process orchestration in the
Rust integration tests or `xtask`, not a new automation shell script.

| Axis | Required cases on Linux, macOS, and Windows |
| --- | --- |
| Global state | Absent directory; missing home resolution; unreadable parent/config; readable but unwritable directory and mirrors; malformed YAML/semantic config; future unsupported authority; valid available host; mixed availability. |
| Paths | Mounted root differs from configured root and metadata name; nested repo CWD; explicit matching/different workspace name; spaces/Unicode; case collisions; inaccessible symlink/reparse target; external Git dir/alternates. |
| Read commands | Bare status, st/diff/log/repo ls/doctor, JSON and plain output, no network, no locks/global writes, read-only workspace succeeds. |
| Describe | Actual mounted manifest changes; text/clear/positional behavior retained; user guidance content preserved; generation failure reported; unrelated workspace unchanged. |
| Fetch/sync | Host mirror path; direct absent/denied/read-only reasons; prune/no-prune; no origin; wrong identity; custom unsafe refspec; changed origin between planning/execution; mirror failure never direct-retries; propagation failure; healthy peers continue. |
| Sync/repo removal recovery | Dirty/wrong branch; merge/rebase conflicts; resume; abort confirmations; dry run; fetch failure prevents sync; existing exit-code precedence; direct transport removal safety; force semantics; registered and unregistered member removal; metadata/guidance update failure. |
| Add branches | Workspace branch exists remotely; fresh branch from remote default; explicit branch override; missing default/unborn remote; URL vs accessible shortname; template rejection before mutation; no-fetch skips mirror refresh but does not prohibit initial clone network. |
| Add races | Two adds of same identity; different identities with same basename; case-fold collisions; concurrent describe; metadata changed during clone; non-wsp process creates final destination immediately before publish. |
| Add interruption | Kill during clone, before publish, after publish/before metadata save, after metadata/before guidance; rerun converges without deleting user/peer directories or losing membership. |
| Add failures | Disk/permission errors in clone, exclusive rename, manifest save, guidance write; existing non-Git dir; symlink; matching adoptable clone; remote/branch mismatch; missing clone already in membership. |
| Side effects | No registry/template/mirror/approval writes; no global hints/version; setup absent/unapproved/unavailable/policy-skipped; no repeat setup on retry; temporary directories stay inside A. |
| Returning to host | Sandbox-added clone remains unregistered and visible to every supported workspace command; st/repo ls/diff/log/describe/repo add/repo rm/fetch/sync/exec work; usable matching mirror remains usable; absent mirror direct-fetches with reason; doctor classifies valid unregistered membership as informational; no silent registration, remote rewrite, directory move, or layout normalization. |

The release fixture must execute these transitions in one sequence, rather
than as independent fresh fixtures:

| Priority | Transition | Required result |
| --- | --- | --- |
| P0 | Host → isolated add URL → host retry of the same add → isolated again | The member, path, remote, branch, and workspace settings persist. Host retry is `already_present` and creates no registry/mirror/setup/discovery side effects. |
| P0 | Isolated add → host adds a different URL in the same request | Existing local member stays byte-for-byte unchanged; only the genuinely new host member receives host registration/mirror effects. JSON reports each result separately. |
| P0 | Isolated clone publication succeeds but metadata/guidance fails → host retry | Compatible clone is adopted without remote/branch/config rewrite, registration, or setup replay; metadata and derived files converge from the latest snapshot. |
| P0 | Isolated same-basename additions → host add/remove/doctor | All surviving clones retain exact directory mappings; status, fetch, sync, exec, and generated guidance resolve those mappings. |
| P0 | Isolated add → host `doctor` and `doctor --fix` | Valid unregistered membership is informational. Neither command registers, rewrites, or removes it. |
| P0 | Explicit host `registry add` after isolated add → isolate again | Global registration/mirror creation is explicit; the existing clone, commits, remote, and membership remain unchanged. Both modes continue working. |
| P0 | Registry/mirror is removed, stale, unreadable, corrupt, or points to another identity between transitions | Matching valid direct origin remains usable when infrastructure is absent; corrupt/mismatched selected infrastructure fails explicitly without remote rewrite or unsafe fallback. |
| P0 | Local/host concurrent add, remove, or describe against the same physical workspace | No peer clone or metadata/guidance update is lost. Conflicting requests fail cleanly; compatible retries converge. |
| P0 | Direct refresh or Git inspection fails during `repo rm` | Normal removal blocks when safety cannot be determined; `--force` retains only its documented override. Failed deletion never falsely reports membership removal. |

Environment variables alone are not isolation. Add an access-denied sentinel
and prove a child running with the same restrictions cannot read/write it.
Linux CI should use an unprivileged process plus filesystem confinement (for
example a mount namespace/bubblewrap fixture with readonly runtime and only A
writable); a root process bypassing chmod is not a valid unreadability test.
On macOS use available process sandboxing and/or an isolated test identity/ACLs;
on Windows use a restricted test identity/token and ACLs. Read-only attributes
alone do not enforce directory isolation on Windows. Keep capability tests
portable and explicitly record platform-specific enforcement. A missing
isolation backend is an unmet required CI gate, not a passing skipped test.

Prefer barriers/controlled Git wrappers/failure injection in private test
helpers over sleeps. Do not add undocumented production environment switches
that bypass ownership or permission checks. Normalize canonical paths on
macOS, account for Windows open-file/rename rules, and restore fixture ACLs
before cleanup. Prove each new regression test fails when the behavior it
protects is intentionally broken, per repository guidance.

## Validation, rollout, and decisions

For each implementation slice run its focused Rust/binary tests, inspect the
diff, and run the applicable formatting/lint checks. After the complete change:

```text
just fix
just skill
just test
just validate
pwsh scripts/smoke.ps1 -Wsp ./target/release/wsp -Offline
just ci
```

Use the repository's actual build prerequisites and scripts; record unavailable
toolchains or platform runners as unverified work rather than claiming success.
CI must run real binaries and isolation gates on the relevant OSes. Run the
normal host regression suite too, especially agent contract, sync conflicts,
shell behavior, mirror propagation warnings, and GC tests. No new setup or
global-state writes may appear merely because output is non-JSON.

Ship workspace-first inspection/describe changes first if independently useful;
ship transport and local add only with their corresponding contracts and
fixtures. Treat the complete documented journey as the release gate for
"agent-ready workspace-local operation." Close #168 and remove its roadmap
section only when that gate passes. The issue's broader adapter/parallel-agent
ideas are separate deliverables.

Decisions made by this plan:

- Automatic context/capability selection; no new mode flag or shadow store.
- Actual detected root wins over host location; explicit different workspace
  targets never silently redirect to CWD.
- Direct origin transport only for infrastructure unavailability, never as a
  retry policy for failed network operations or malformed state.
- Existing clone directories stay put during local add; publication never
  replaces a destination; interruptions recover by inspection and rerun.
- Local add skips arbitrary setup execution and global template imports with
  separate results; no approvals are inferred from editable workspace files.
- `doctor --fix` is deferred; portable `repo rm` uses the same safe direct
  transport fallback as fetch and sync.
- JSON changes are additive and samples/generated instructions ship together.

Implementation gates requiring an explicit design resolution if unmet:

- A safe exclusive directory publication implementation for every supported
  OS/filesystem, without introducing unsafe code into these crates.
- Non-mutating permission classification under mounts and ACLs; unknown
  capability must not accidentally restore global writes.
- #170 compatibility for fixed existing directory mappings and any additional
  persisted state proposed during implementation. No journal is approved here.
- Enforced isolation in CI, including permission-denied tests that actually
  deny access and cannot be bypassed by the test identity.
- If setup must run during local add, design its authorization and replay
  semantics separately before changing this plan's skip policy.

These are targeted gates for the relevant phases, not reasons to block the
independent mounted-path and read-only work already understood.
