# Resumable `wsp sync` conflict handling

**Status:** Implemented
**Date:** 2026-05-10

## Goal

Make `wsp sync` safe and resumable across multiple repositories. A conflict in
one repository must preserve the user's partial work, must not prevent other
repositories from syncing, and must not require a separate recovery command or
wsp-owned journal.

## Design

Git's in-progress operation markers are authoritative:

- `.git/rebase-merge` or `.git/rebase-apply` means a rebase is in progress.
- `.git/MERGE_HEAD` means a merge is in progress.

`wsp sync` processes every repository independently:

1. Fetch mirrors and propagate updated refs to workspace clones.
2. If a repository has an in-progress rebase or merge, ask Git to continue it.
3. If the operation still has unresolved conflicts, report it as paused and
   leave Git's state untouched.
4. If no operation is in progress, run the configured sync strategy normally.
5. Continue through the remaining repositories regardless of paused peers.

The user workflow is therefore the same command each time:

```text
wsp sync
# resolve and stage conflicts in the affected repositories
wsp sync
```

`wsp sync --abort` remains available because aborting discards an in-progress
operation and should be an explicit choice.

Dry runs never continue an operation. They report in-progress repositories as
paused and preview clean repositories.

## Output contract

Each repository has one of three statuses:

- `ok`: the repository completed or needed no update.
- `paused`: Git has an unresolved rebase or merge that can be resumed.
- `failed`: a hard error prevented progress.

JSON includes both `status` and the existing `ok` boolean. `ok` remains for
compatibility; `status` carries the complete state.

The process exit code is:

- `0` when every repository is `ok`.
- `1` when at least one repository failed and none are paused.
- `2` when at least one repository is paused, even if another failed.

Exit code 2 intentionally distinguishes work requiring conflict resolution
from a hard failure. Callers needing per-repository detail use `--json`.

## Safety properties

- Failed rebases and merges are never automatically aborted.
- A rerun does not restart an operation against a newly fetched target; Git
  continues the operation it already recorded.
- The dirty-tree guard is evaluated only when no operation is in progress, so
  conflict files are not misreported as ordinary uncommitted changes.
- No wsp-specific state is written inside a clone.
- wsp does not infer whether an operation was started by wsp or directly by the
  user. Invoking the mutating `wsp sync` command explicitly authorizes Git to
  continue any operation it finds; `wsp st` and other read-only commands do not.

## Verification

Tests cover:

- Rebase and merge conflicts preserving their in-progress state.
- Successful continuation after conflicts are resolved and staged.
- Failed continuation retaining the paused state.
- Clean repositories continuing after a peer conflicts.
- Rebase and merge parity.
- Structured output and exit-code precedence.
- Dirty-tree, wrong-branch, and repository-error guards.

See [ADR-001](../ADR/001-sync-conflict-resolution.md) for the accepted contract.
