# Quint design for portable workspace ownership

**Status:** Standalone model design; no model execution or implementation claim.
**Date:** 2026-09-18
**Author:** Astra
**Related:** [workspace-local operation plan](2026-09-18-workspace-local-sandbox-operation.md), issue #168.

## Purpose

Specify how the product should behave when the same workspace alternates
between isolated and host invocations. The specification must be useful as an
independent oracle for future generated traces against the real binary. It
must not simply transcribe the current Rust control flow and certify its own
assumptions.

The initial deliverable is a bounded Quint state machine for ownership,
membership, publication, interruption, retry, and removal. Git supplies opaque
facts such as clone identity and whether removal is safe. Git's object graph,
merge algorithm, operating-system permission enforcement, and concrete CLI
serialization remain separate executable-test obligations.

### Current executable-model boundary

This document describes the intended model. The checked-in
`formal/quint/workspace_local.qnt` deliberately implements a smaller initial
slice. Its repository-to-slot mapping is fixed per identity: the two `api`
repositories exercise distinct stable mappings, but it does not yet enumerate
candidate paths, model an occupied destination, or explore allocator races.
Likewise, the files in `formal/quint/faults/` are standalone minimal faulty
models used to verify that the Quint runner reports the selected invariant;
they are not source mutations of the normative model. Their counterexamples
do not demonstrate mutation sensitivity of the normative allocation,
publication, or concurrency transitions. The model README is the authoritative
inventory of implemented coverage and omissions until those mechanisms are
added.

This document deliberately precedes a `.qnt` implementation and replay adapter.
It proposes no new product commands, flags, journal, or local registry.

## 1. Initial scope and boundaries

The first executable model should include:

- Two physical workspaces A and B with the **same metadata name**, and a host
  workspace index that resolves that name to B. A is the mounted workspace.
- Three repository identities: `alpha/api`, `beta/api`, and `alpha/web` on one
  synthetic host. The two `api` identities collide by basename.
- Two invocation slots, each with independently captured capabilities, command
  request, phase, and mounted root. This allows a host invocation and an
  isolated invocation to overlap on A without pretending that context is a
  single mutable property of the workspace.
- URL and unambiguous shortname requests, existing-member retries, compatible
  clone adoption, ordinary host additions, and a mixed two-repository add.
- Explicit host registration, conservative removal and forced removal,
  metadata description updates, derived guidance generation, diagnostic reads,
  and crash/restart at persistent boundaries.
- An abstract refresh sub-operation used by removal: direct or mirror choice,
  identity validation, network success/failure, and no retry with a different
  transport after a selected transport fails.

Do not put merge/rebase state machines, all sync modes, setup execution
internals, URL parsing, general templates, arbitrary `exec`, workspace
creation/rename/deletion, or global GC in this first model. They are not needed
to expose ownership and retry bugs. Subsequent models may compose with the
ownership model, but the initial scope must remain bounded and reviewable.

Read operations are represented by one diagnostic/inspection action with a
write-free contract, then mapped to concrete commands by tests. `exec` is
excluded from this write-free contract: it executes caller-authorized arbitrary
code. `describe` is a workspace mutation, not a read.

## 2. Authorities and finite domains

Use small algebraic types and total finite maps in Quint. Absence must be an
explicit variant, not a magic empty string. Avoid unbounded timestamps,
monotonically increasing event counters, and unconstrained strings.

| Domain | Initial values |
| --- | --- |
| `Root` | `Mounted`, `Sibling` |
| `Repo` | `AlphaApi`, `BetaApi`, `AlphaWeb` |
| `Process` | `P0`, `P1` |
| `BranchIntent` | `Feature`, `Other` |
| `Checkout` | `ExpectedBranch`, `OtherBranch`, `Detached` |
| `Slot` | `Api`, `BetaApiSlot`, `HostBetaApiSlot`, `Web`, plus one foreign occupied slot |
| `Access` | `Absent`, `Denied`, `ReadOnly`, `Writable`, `Unknown` |
| `Validity` | `Valid`, `Malformed`, `Unsupported` |
| `MirrorState` | `Absent`, `Matching`, `WrongIdentity`, `Corrupt` |
| `Safety` | `Safe`, `Dirty`, `Unmerged`, `Detached`, `Unknown` |
| `Transport` | `None`, `Direct`, `Mirror` |
| `Outcome` | `Pending`, `Success`, `AlreadyPresent`, `Adopted`, `Blocked`, `Partial`, `Crashed` |

Use a finite ordered candidate list per identity for path allocation. Existing
member mappings are immutable while membership persists. Candidate exhaustion
is a clear failure, not permission to replace another directory. Treat slot
equivalence as a parameter: one profile uses exact names; another identifies
case-fold-equivalent names. Real Unicode/filesystem naming is not modeled.

Repository identity abstracts the result of canonical URL parsing. URL aliases
may map to one identity, but actual URL equality remains a separate observable:
an existing clone's literal origin must not be rewritten to a registry alias.

## 3. State

Separate persistent authorities, volatile process state, environment facts,
and specification-only observations.

### Persistent workspace state

For each physical root:

- `manifest`: validity plus membership `Repo -> Optional<Member>`.
- `Member`: recorded directory slot and branch intent. Description is a
  separate small value, so concurrent description changes can expose stale
  metadata writes.
- `directories`: `Slot -> DirectoryState`, where a directory is absent,
  foreign, incomplete, or a complete ordinary clone.
- `Clone`: canonical origin identity, literal URL token, checkout state,
  content token, user configuration token, and safety facts. The content token
  stands for commits/index/worktree data that a preserving operation must not
  replace. Invalid storage is an explicit clone-validation result.
- `guidance`: the membership/mapping/description projection last generated,
  plus a separately preserved user-section token. It may be stale after a
  crash. Staleness is observable and repairable, not authority over membership.
- `staging`: process-owned incomplete or complete staged clones. A crashed
  invocation can leave staging behind; staging is never membership and never
  authorizes deletion of a final destination.

Do not add a durable `was_added_in_sandbox` flag. Real workspaces have no such
provenance authority. An ordinary compatible unrecorded clone must receive the
same recovery policy regardless of whether it came from a crashed local add,
a crashed host add, or a user-created clone.

### Persistent global state

- `registry`: `Repo -> Optional<UrlToken>`.
- `mirrors`: independent `Repo -> MirrorState`. A mirror can exist without a
  registry entry after interruption; registration can exist without a usable
  mirror. Never assert that their domains are equal.
- Abstract template, approval, setup, default-application, and advice effects,
  recorded as attributed events for the current transition. Preserve a small
  baseline token for stores that no in-scope operation may mutate.
- The host workspace index, mapping the common metadata name to `Sibling`.

Registration is not workspace membership. Neither registry nor mirrors may
change workspace paths, origin URLs, branches, or content.

### Environment and process state

Each process captures `root`, requested explicit workspace selector,
per-store access/validity, command, ordered requests, phase, chosen transport,
read snapshot, staged clone, and per-repository result. Capabilities are inputs
to policy; actual reads/writes may still fail after capability observation.

An isolated profile denies every global capability. A readable profile allows
registry lookup but denies global writes. A host profile permits required
global effects. Mixed capability profiles vary registry and mirror access
independently. Malformed readable global configuration blocks ordinary commands
before mutation; doctor can return diagnostic failures without mutation.

Model lock ownership explicitly for metadata publication/recheck and guidance
generation. Slow clone/refresh work happens outside the metadata lock.
`Crash(p)` releases OS locks and volatile state, preserving completed writes.

### Ghost observations

Maintain only what the checker needs: the previous persistent projection,
last action and attributed effects, admission decision, and short-lived
operation snapshots. These support transition invariants and trace export.
Ghosts must never influence guards for recovery or change product behavior.
In particular, a retry cannot consult the dead invocation's branch snapshot,
local/host provenance, deletion receipt, or skipped-setup history.

For exhaustive checking, reset observation records after checking each step;
do not accumulate an unbounded event history in model state. Trace output is
the simulator's sequence of states, not a modeled growing list.

## 4. Actions and commit points

Each action below is small enough to expose a meaningful interruption or
interleaving. The `.qnt` implementation should use named actions with explicit
arguments, not one opaque nondeterministic `command` action.

### Invocation and reads

`Begin(p, root, capabilities, command, args)` detects and validates the mounted
manifest before depending on optional globals. No selector, or an explicit
selector equal to detected metadata name, selects the mounted physical root.
An explicit different name requires host authority and resolves through that
authority; it must never silently target the mount.

`Inspect(p)` leaves persistent state unchanged and returns membership plus
diagnostic facts. Valid unregistered members are supported. Missing/mismatched
clones and stale guidance are distinguishable diagnostics. Local `doctor --fix`
is rejected before repair; host doctor may not auto-register valid members.
The first model does not implement other host doctor repairs.

### Add

1. `ResolveBatch`: resolve all request identities/branch intents before effects.
   A shortname may resolve from existing membership without registry access;
   a genuinely new shortname requires a readable unambiguous registry URL.
   Ambiguous names and conflicting branch requests fail before mutation.
2. `InspectExisting`: under current metadata, an existing valid member returns
   `AlreadyPresent` without clone/global changes. An explicit conflicting
   **stored intent** fails. A developer's changed current checkout is not a
   reason to rewrite it on an ordinary retry. Missing or invalid member clones
   block retry instead of being silently re-cloned.
3. `InspectRecoverable`: if not a member, inspect the candidate destination. A
   compatible ordinary clone on the requested branch can be adopted, including
   its dirty worktree; preserve its contents/config/origin. An incompatible,
   foreign, unsafe, or wrong-branch destination blocks without replacement.
4. `AdmitNew`: only after the preceding checks can a genuinely new host add
   authorize normal registration/mirror preparation for that identity. Local
   admission never authorizes global effects. Record the decision in volatile
   process state for attribution, not a persistent journal.
5. `PrepareHostInfrastructure`: separately publish a valid matching mirror and
   update registry under its own lock. Expose interruption between the writes.
   Failure may leave partial global infrastructure but never workspace changes.
6. `StageClone`: clone by direct URL locally or selected host source into a
   process-owned staging slot. Network failure may leave an incomplete stage.
   Stage completion requires clone and requested-branch validation.
7. `RecheckAndPublish`: acquire metadata lock, reread latest manifest, recheck
   branch intent and membership, recompute only the new destination, and
   atomically publish without replacement. If a peer already committed a
   compatible member, preserve it and discard only this process's staging.
   If a compatible destination already exists, adopt rather than replace it.
8. `CommitMembership`: while holding that same metadata lock, atomically update
   the latest manifest with the new mapping/member, preserving all peer changes.
   Failure leaves a published clone without membership. Never compensate by
   deleting the published clone, which may already contain user work.
9. `HostExtras`: initial host-created members may receive existing host policy
   effects. Local creations, adoptions, and existing-member retries never
   replay setup/import/default application to existing clones. A crash after
   membership but before extras does not authorize replay on retry.
10. `GenerateGuidance`: acquire metadata lock, read its latest state, replace
    managed guidance, and preserve user sections. Do not write a stale snapshot
    obtained before another member or description committed.
11. `Finish`: return per-repository stages and overall outcome. An earlier
    successful member stays committed if a later member fails. Failed guidance
    after membership is partial success, not rollback.

`Crash` is enabled after each persistent step and during staging. Retrying is
a fresh `Begin`, possibly with different capabilities. No recovery transition
may consume dead-process state.

### Explicit registration

`Register` requires host authority and URL identity validation. Model mirror
publication and registry commit as separate persistent steps, with failure
between them. Every step preserves **all workspace state**, including clone
config and generated guidance. Repeating registration may repair its own
global infrastructure according to the existing command contract.

### Refresh and removal

`SelectTransport` validates the actual clone identity and storage. Select an
existing valid matching mirror when write/use authority permits it; absent or
inaccessible infrastructure permits direct origin. Corrupt/wrong-identity
selected infrastructure is an error, not an absence. Once `Refresh` fails for
the selected transport, finish blocked; never add a direct retry transition.

`PreflightRemove` resolves every requested identity from current workspace
membership, checks unique directory ownership, and checks safety. Without force,
dirty, unmerged, detached, unknown, or failed refresh blocks deletion. A mirror
refresh can already have changed host mirror state before removal is blocked;
"blocked removal preserves everything" would therefore be a false invariant.
The protected state is membership, clone contents, and directory ownership.

`RecheckRemove` acquires metadata lock and verifies the selected member,
directory, and branch intent still agree with its snapshot. Force bypasses Git
safety uncertainty, never workspace-root containment or directory ownership.

For each selected member, separate:

- `DeleteClone`: success, pre-delete failure, or partial filesystem deletion.
- `CommitRemoval`: atomically remove that member/mapping from latest metadata.
- `GenerateGuidance`: update derived state from the latest manifest.

A crash after deletion but before manifest commit legitimately leaves a member
whose clone is missing. A filesystem error can leave a damaged clone. Keep the
membership and report the failure; do not claim rollback. Normal retry must
fail nondestructively when safety cannot be established. An explicit forced
retry can clear an owned missing clone's metadata after revalidation.

Partial batch success is committed per member. A repeated batch containing
already-removed identities is allowed to return a clear membership error; the
initial model must not invent an idempotent remove promise absent from the CLI.
The caller can retry remaining members with explicit force when needed.

### Description and external changes

`Describe` updates the latest manifest under lock, then regenerates guidance.
It must not erase membership committed during earlier slow clone work.

Small environment actions create foreign occupied paths, modify a clone's
opaque user token, change its current checkout/origin validity, and remove or
corrupt optional global mirrors. These expose revalidation requirements.
Separate controlled concurrency from unrestricted hostile filesystem mutation:
cooperating wsp operations honor locks; arbitrary user Git writes do not.

The baseline removal profile assumes no arbitrary Git writes between the final
safety check and deletion. An adversarial profile relaxes this assumption and
should expose the check/delete race. Metadata locking alone does not solve it.
Do not advertise the baseline as a proof against unrestricted concurrent Git
edits. Any stronger product guarantee needs a concrete mechanism and refinement
test before strengthening the model.

## 5. Invariants and properties

Express transition properties using previous state plus the last attributed
action. A global process may mutate globals while a local process is active;
therefore compare effects attributable to the local action, not global state
at the beginning and end of a concurrent local invocation.

| Property | Required assertion |
| --- | --- |
| Authority confinement | Every local action's writes are within its captured physical root. No local action mutates any global store or the sibling root. |
| Root selection | Same-name explicit/implicit invocations select the detected mounted root even when the host index resolves that name elsewhere. |
| Stable mappings | Every member present before and after a transition retains its directory mapping, unless an explicit future relocation action is modeled. Removing/re-adding a member starts a new lifetime. |
| Unique ownership | Valid metadata never assigns equivalent directory slots to two members. Invalid initial ownership causes rejection, not destructive repair. |
| Exclusive publication | Publication changes an absent slot to this process's complete clone; it never overwrites foreign, peer, or already-published content. |
| Existing-member preservation | Existing-member add changes no clone content, origin, branch, configuration, registry, mirrors, setup, template, or approval state for that identity. Guidance may repair a reported stale projection. |
| Adoption preservation | Adopting a compatible unrecorded clone changes only membership/derived files; it does not register, rewrite, reset, or run host extras. |
| Explicit registration isolation | Every registration step leaves all workspace persistent projections unchanged. |
| Local visibility | Successful membership commit remains visible across context switches until explicit successful removal. Registry absence never hides it. |
| Fresh metadata updates | A commit preserves every unrelated field/member written before its lock acquisition. No stale read snapshot replaces current metadata. |
| Guidance consistency | Successful guidance generation matches metadata at its commit point and preserves user text. Crashes can leave stale guidance, which reads report without rewriting. |
| Conservative removal | No unforced deletion occurs without established safe facts and successful refresh for the selected clone. Force never grants ownership of a foreign path. |
| Removal accounting | Metadata removal requires successful deletion or forced confirmation of an already-missing owned clone. Delete failure does not report successful member removal. |
| Transport identity | Every refresh uses a validated source for the member identity and preserves literal origin configuration. |
| No transport retry | A failed selected mirror refresh cannot be followed by direct refresh within that operation. A new invocation may re-evaluate changed infrastructure. |
| Read purity | Diagnostic/inspection actions have no persistent effects, including guidance, advice, locks that persist, and registry repair. |
| Result truthfulness | Reported stages correspond to committed events; killed processes need not emit final JSON. A failed required stage is not overall success. |

Never require `member iff clone exists` at every state. Published orphan clones,
members with deleted clones after removal interruption, and damaged clones
after partial deletion are reachable and must remain representable.

Also avoid universal liveness under unlimited crashes/network failures. Check
conditional recovery properties: once faults cease, a compatible published
clone with an available candidate and unchanged branch intent can be adopted;
committed membership with stale guidance can regenerate; a missing clone after
interrupted removal can be explicitly force-cleared. These are finite recovery
scenarios initially. If temporal fairness checking is later added, state the
scheduler, successful-I/O, and no-further-external-change assumptions explicitly.

## 6. Concurrent host admission: precise promise

Sequential host retry of a local member must cause no global effects. A host
add admitted as genuinely new before a concurrent local add commits is a
different case: it may already have registered/fetched before noticing the
peer member. The model should permit those **already-authorized** host effects
while requiring preservation of the winning clone and no replay of extras on
it. Linearize host eligibility at its authoritative absence/adoption check.

This is a deliberately explicit interpretation of the product contract, not
an assertion that the current code fully implements it. If the desired promise
is that a concurrently winning local add retroactively forbids all host
registration, a reservation/serialization design is required; a second check
after network I/O cannot undo already-visible global effects. Do not hide that
decision by making registration and publication one magical atomic action.

Similarly, recovery must not infer inaccessible provenance. If collision
changes make an orphan clone no longer discoverable at the allocator's candidate,
safe nondestructive failure is acceptable in the first model; unconditional
adoption convergence is not. Add a targeted orphan/collision trace and document
whether future recovery should inspect additional compatible candidates.

## 7. Trace and future replay contract

Use ITF from Quint as the model interchange format, and normalize it into a
versioned product trace. Keep raw ITF, seed, model revision, profile, and failing
step alongside normalized traces. The trace should distinguish environment
fixtures, invocation starts, scheduling barriers, product observations, and
assertions. Internal model steps are not automatically individual CLI commands.

Example normalized schema (illustrative, not a new product JSON envelope):

```json
{
  "schema_version": 1,
  "profile": "three_repos_two_contexts",
  "seed": "fixed-seed",
  "fixtures": {"roots": ["mounted", "sibling"], "same_name": true},
  "steps": [
    {"kind": "invoke", "id": "a", "root": "mounted", "access": "isolated",
     "command": "repo_add", "repos": ["alpha_api"], "form": "url"},
    {"kind": "barrier", "id": "a", "point": "clone_published"},
    {"kind": "crash", "id": "a"},
    {"kind": "invoke", "id": "b", "root": "mounted", "access": "host",
     "command": "repo_add", "repos": ["alpha_api"], "form": "url"},
    {"kind": "expect", "id": "b", "outcome": "adopted",
     "unchanged": ["global_stores", "clone_origin", "clone_user_state", "sibling"],
     "members": {"alpha_api": "api"}}
  ]
}
```

Replay maps symbolic identities to controlled real Git remotes, roots to fresh
temporary directories, and abstract content tokens to observable files/commits.
Use the binary for every product action. Independently read manifests, clone
origins/HEAD/refs/worktree/config, global file inventories, and sibling state.
Do not use the implementation's directory allocator or transport planner to
compute expected results. Normalize timestamps, temporary paths, and Git object
IDs by explicit semantic mappings, never by dropping surprising differences.

Compare only specified observable fields; leave incidental formatting and map
iteration order unconstrained. Preserve exact existing origin bytes and
workspace mappings where the contract requires exact preservation. Test JSON
stages/exit codes as well as independent filesystem facts: successful JSON is
not evidence that the intended filesystem transition happened.

Crash barriers require a reliable future test seam. A wrapper around Git can
control clone/fetch phases, but cannot reliably stop precisely after clone
publication or manifest replacement. Use a narrowly scoped test-build event
channel or equivalent deterministic harness if those barriers are implemented;
do not add production environment variables that weaken validation or security.
Until a barrier exists, mark that trace **not replayable**, not passing. Pure
model execution remains useful but does not validate Rust refinement.

Use Rust integration tests for binary replay and `xtask` for repository-wide
generation/validation orchestration, invoked by `just`. No replay machinery is
part of the current standalone design deliverable.

## 8. Exploration and adequacy

Start with scenario runs and simulation of the same model before attempting a
large cross-product of state variables. Pin the Quint version when adding code.
Typecheck, run named deterministic scenarios, then run seeded randomized traces
against all safety invariants. Export minimized counterexamples as durable
regression traces. Bounded verification, if the available backend supports the
chosen finite model, supplements simulation; do not call random runs exhaustive.

Use separate small profiles rather than enabling every axis simultaneously:

1. One process, two repos: isolated add → host retry → explicit registration →
   isolated reuse → removal. Include user edits before the retry.
2. Two processes, two colliding repos: same/different identity publication,
   concurrent describe, host admission racing local publication.
3. One process, two repos: crash at every add/removal persistent boundary,
   retry in the opposite context, then fault-free recovery.
4. One process, one repo, two same-name roots: routing and sibling isolation.
5. One process, one repo: mixed capability/validity and refresh source states.

Set initial random trace bounds around 40–80 small actions so a crash/retry
journey can complete. The exact sample count is a reproducibility parameter,
not a completeness claim. For bounded checking, increase depth until every
named phase and its opposite-context retry is reachable; report actual explored
depth/state count and backend result.

Require witnessed coverage for every failure boundary, each add disposition,
both transports, force/nonforce removal, all mirror states, basename collision,
both contexts, and both-process interleavings. A run with no violated invariant
but no witness reaching membership commit is inadequate.

Prove the oracle is useful with explicit faulty variants: register before
member check; recompute existing mappings; overwrite occupied publication;
apply stale metadata snapshot; clear metadata after failed deletion; route by
host name instead of mounted root; fall back after mirror failure; replay setup
on adoption; regenerate guidance from a stale snapshot. Each must yield a
counterexample or a documented limitation in the model. Keep mutations outside
the normative specification so production guards are not assumed by properties.

## 9. Real-code obligations and known limits

The model abstracts these as facts or primitive outcomes. Conventional tests
must establish them against the operating system and Git:

- Non-replacing publication on Linux, macOS, and Windows; atomic manifest
  replacement; lock release on process death; partial filesystem deletion.
- Permissions/ACLs and enforced filesystem confinement, including readonly
  globals, unavailable HOME, sibling denial, and proof that the child identity
  cannot read or write a denied sentinel.
- Symlinks/reparse points, `.git` files, `commondir`, alternates, refs/objects
  escapes, URL identity canonicalization, unsafe refspecs, and changed origins.
- Real fetch/prune effects, no direct network retry after mirror failure,
  default-branch detection, removal branch-safety classification, sync conflicts,
  and read commands that neither refresh the index nor lazily fetch objects.
- Setup/import error reporting, byte-preserving user guidance sections,
  stdout JSON validity, exit precedence, and plain output on all supported OSes.
- Power loss and storage durability. Process-crash modeling assumes completed
  atomic writes survive process death; it does not prove fsync/power-loss safety.

The implementation currently has separate add, context, transport, and removal
paths. The model's shared ownership and transport invariants intentionally apply
across those paths; existing implementation differences are candidates for
counterexamples, not reasons to weaken the model. Current real-binary tests
provide useful host-return happy paths, but do not substitute for deterministic
crash/interleaving witnesses or enforceable confinement.

## 10. Completion criteria for the isolated modeling phase

The next modeling-only change is complete when a reviewer can inspect a small
Quint source plus README, reproduce typechecking and named scenario runs, obtain
seeded exploration results, and see counterexamples for the faulty variants.
Document every deliberately unsupported replay barrier and unresolved product
policy. No product code, new runtime state, or replay adapter is required to
finish that phase, and a green model must not be presented as proof that the
current binary conforms.
