# ADR-001: Sync conflict resolution — exit code 2 and mid-flight state

**Status:** Accepted
**Date:** 2026-05-12
**Deciders:** @jganoff
**Plan:** [2026-05-10 wsp sync conflict resolution](../plans/2026-05-10-wsp-sync-conflict-resolution.md)

## Context

`wsp sync` automatically aborted in-progress rebases and merges when a conflict occurred, destroying any partial resolution work. Users had to `cd` into each repo and re-initiate the rebase manually. There was no mechanism to resume a paused sync. CI scripts and tooling expected only exit codes `0` (success) and `1` (error), with no distinct code for the resumable/paused state.

## Decision

Two related changes were shipped together — both Type 1 (one-way door) once CI scripts adopt the new contract.

### 1. Exit code 2 for paused/resumable state

`wsp sync` uses a three-value exit code contract:

- `0` — all repos synced cleanly
- `1` — hard failure (network error, missing branch, configuration error)
- `2` — one or more repos paused with unresolved conflicts (resumable by rerunning `wsp sync`)

Exit code `2` is emitted when any `SyncRepoResult` has `status: Paused` at the end of the invocation. The precedence rule is: `2` beats `1`; either beats `0`. This is implemented in `output::exit_code`.

Exit code `2` collides with the POSIX convention of using `2` for "usage error" (as in `grep`, `diff`, `bash`). This divergence is documented in `docs/usage.md` and `CHANGELOG.md`. Exit code `3` was rejected (see Alternatives).

### 2. Leave repos mid-flight on conflict

When `git rebase` or `git merge` produces a conflict, `wsp sync` no longer calls `git rebase --abort` or `git merge --abort`. The repo is left in its mid-flight state:

- Rebase conflict: `.git/rebase-merge/` directory is present
- Merge conflict: `.git/MERGE_HEAD` file is present

These on-disk markers are the sole source of truth — no additional state file is written. Each `wsp sync` probes `in_progress_op(dir)` to discover mid-flight repos at invocation time. It resumes only operations whose Git-recorded source branch matches the workspace branch; other operations are left untouched and reported as failures.

If an operation cannot continue because conflicts remain, the repo is reported as `Paused`, clean repos are synced normally, and the overall exit code is `2`.

## Consequences

### Positive

- Users no longer lose conflict-resolution work to automatic aborts.
- Stateless design: git on-disk markers (`.git/rebase-merge`, `MERGE_HEAD`) are the single source of truth. No `.wsp/` state file that can drift.
- Recovery works from any shell without wsp-managed state — the user resolves conflicts with Git and reruns `wsp sync`.
- `SyncRepoStatus` enum (`Ok`/`Paused`/`Failed`) eliminates the previous illegal-state combination of `ok=true, paused=true`.

### Negative

- Breaking change for CI scripts that check `$? -eq 1` to detect any sync failure: they now need to handle `2` separately. Documented in CHANGELOG.
- Exit code `2` collides with POSIX "usage error" convention in some tools. Documented in `docs/usage.md`.
- `wsp sync` cannot distinguish an operation it started from one the user started directly on the workspace branch. Git's recorded source branch scopes continuation, so an operation on another branch remains the developer's to resolve or abort.

## Alternatives considered

- **Keep auto-abort, improve only the footer.** Rejected: the footer change is cosmetic — it doesn't solve the primary pain of losing partial rebase state.
- **Persist sync state to `.wsp/sync-state.json`.** Rejected: dual source of truth that can drift out of sync with git's on-disk state. Git's markers are authoritative.
- **Exit code 3 to avoid POSIX collision.** Rejected: no established convention, less discoverable, and strategy documentation already specifies `2`.
- **Refuse all work when a repo is mid-flight.** Rejected: independent repositories can still make progress, and unresolved repos remain safely paused.
- **Add a `--continue` recovery flag.** Rejected: the resumability tenet says to rerun the same command, and the extra mode duplicates Git's recovery vocabulary without resolving ownership ambiguity.

## Reversibility

Both decisions are recoverable but require user-visible contract changes:

- **Exit code 2 → 1:** requires a major version bump and CHANGELOG entry. CI scripts that already adopted `2` would need updating.
- **Mid-flight state → auto-abort:** is a small change in `rebase_onto`/`merge_from` in `git.rs`, but it would discard partial conflict-resolution work.

## References

- Plan: `docs/reference/plans/2026-05-10-wsp-sync-conflict-resolution.md`
- Exit code logic: `crates/wsp/src/output.rs` — `exit_code` function
- Mid-flight detection: `crates/wsp-core/src/git.rs` — `in_progress_op`, `rebase_onto`, `merge_from`
- Sync classification: `crates/wsp/src/cli/sync.rs` — `sync_one_repo`, `run_live`
