# Removal Safety & Branch Detection

`wsp rm` and `wsp repo rm` run safety checks before removal. Both `workspace::remove` and `workspace::remove_repos` follow the same pattern:

1. **Uncommitted changes** — `changed_file_count` (dirty working tree) is checked first. If non-zero, removal is blocked immediately. Ahead-of-upstream commits are intentionally **not** checked here — they are handled by branch safety (step 5), which correctly detects squash-merged work even when the remote tracking branch has been deleted.
2. **Linked worktrees** — `list_linked_worktrees` enumerates any linked worktrees (`git worktree add`). Each live linked worktree is checked for uncommitted changes and unpushed commits. Blocked if any are found. Entries that Git marks `prunable` are ignored because their checkout no longer exists; the safety check does not prune them or otherwise mutate the repository. (`git status` on the main working tree is blind to linked worktree changes — this check closes that gap.)
3. **Wrong-branch detection** — If HEAD is not on the workspace branch, the workspace branch is checked for unpushed commits separately. This catches the case where a user checked out `main` but has work on the workspace branch.
4. **Fetch with prune** — fetches the mirror from upstream, then propagates to the clone via path-based local fetch with prune. Updates remote tracking refs and clears stale ones (e.g., branches deleted after a PR merge on GitHub). Also removes the legacy `wsp-mirror` remote if present.
5. **Workspace branch safety** — `git::branch_safety()` in `crates/wsp-core/src/git.rs` evaluates the workspace branch (`meta.branch`) against the default branch (`origin/main`). Returns one of four variants, checked in order:

| `BranchSafety` | Meaning | `wsp rm` behavior |
|---|---|---|
| `Merged` | Branch is ancestor of target (regular merge) | Safe, silent removal |
| `SquashMerged` | Tree matches what a squash-merge would produce, or file contents match (`is_content_merged`) | Safe, silent removal |
| `PushedToRemote` | `origin/<branch>` exists but branch is not merged | **Prompt** — shown alongside any open-PR warning; `--yes` or `--force` bypasses |
| `Unmerged` | Branch only exists locally, never pushed | **Blocked** — requires `--force` |

`PushedToRemote` is treated as a confirmation prompt (not a hard block) because the code is already on the remote and not at risk of loss. It is folded into the open-PR warning when a PR exists, so the user sees one combined prompt covering both. `Unmerged` requires `--force` because the branch is local-only and could be permanently lost.

6. **Current-branch safety** — If HEAD is on a different branch than the workspace branch (e.g. the user switched to `hotfix/urgent` mid-workspace), that branch is also evaluated with `branch_safety()`. This catches the primary data-loss path: workspace branch was merged and pruned, user is on a different branch with unpushed work, and `wsp rm` would have silently removed it without this check.

The behavior differs between `check_removal_blockers` (used by `wsp rm`) and `remove_repos` (used by `wsp repo rm`):

| Function | `PushedToRemote` on current branch | `Unmerged` on current branch |
|---|---|---|
| `check_removal_blockers` | Soft blocker (`pushed_unmerged`) — CLI prompts | Hard blocker — requires `--force` |
| `remove_repos` | Hard blocker — requires `--force` | Hard blocker — requires `--force` |

`remove_repos` uses hard blockers for both because it has no interactive prompt path. `check_removal_blockers` uses a soft blocker for `PushedToRemote` because the code is already on the remote, and the CLI folds it into the open-PR prompt so the user sees one combined confirmation.

**Exception — `PushedToRemote` with unpushed local commits:** When `branch_safety` returns `PushedToRemote` for the current branch but `commit_count("origin/<current>", "<current>") > 0`, the branch has local commits not yet pushed to the remote. This is treated as a hard blocker (`local_unmerged`) rather than a soft blocker, because the un-pushed commits are at risk of loss. Specifically: `PushedToRemote` means the remote tracking ref exists; the additional `commit_count` check catches diverged state where the user has committed locally after the last push but before the PR was opened (or after force-pushing without the local changes).

## Expected Workflow

1. `wsp new my-feature` — creates workspace with branch
2. Make changes, commit, push, open PR (using git directly)
3. PR gets merged (regular, squash, or rebase merge)
4. `wsp rm` — fetches mirror from upstream, propagates to clone (with prune), detects merge via the three-layer check (`branch_is_merged` → `branch_is_squash_merged` → `is_content_merged`), removes workspace

No manual `git fetch` or `git pull` needed — `wsp rm` fetches implicitly via the mirror. If the fetch fails (network issues), the safety check falls back to local data and warns on stderr.

## Edge Case: Squash Merge with Conflict Resolution

If a squash merge resolved conflicts by changing file contents, `is_content_merged` may return `false` because the branch's files don't match what's on `origin/main`. The workspace will be detected as `Unmerged` and blocked. Use `--force` to remove.

## Deferred Deletion (gc)

`wsp rm` moves workspaces to `~/.local/share/wsp/gc/` by default instead of permanently deleting them. This follows git's reflog+gc pattern — users don't know about it until they need recovery:

- `wsp rm` — silently moves to gc.
- `wsp ls --removed` — lists recoverable workspaces with their expiry; `wsp recover <name>` restores one and cds into it.
- `gc::maybe_run()` runs after every command (at most once per hour), purging entries older than `gc.retention-days` (default 7, config key `gc.retention-days`).

The gc dir lives alongside mirrors in the XDG data directory (`~/.local/share/wsp/gc/`). `gc::move_dir` uses `fs::rename` when possible, falling back to recursive copy + delete for cross-filesystem moves (EXDEV). GC metadata (`.wsp-gc.yaml`) is written inside the workspace dir before the move.

`workspace::remove(paths, name, force)` — always moves to gc. There is no bypass.
