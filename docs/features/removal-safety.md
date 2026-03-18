# Removal Safety & Branch Detection

`wsp rm` and `wsp repo rm` run safety checks before removal. Both `workspace::remove` and `workspace::remove_repos` follow the same pattern:

1. **Pending changes** — `changed_file_count` (dirty working tree) and `ahead_count` (unpushed commits) are checked first. If either is non-zero, removal is blocked.
1b. **Wrong-branch detection** — If HEAD is not on the workspace branch, the workspace branch is checked for unpushed commits separately. This catches the case where a user checked out `main` but has work on the workspace branch.
2. **Fetch with prune** — fetches the mirror from upstream, then propagates to the clone via path-based local fetch with prune. Updates remote tracking refs and clears stale ones (e.g., branches deleted after a PR merge on GitHub). Also removes the legacy `wsp-mirror` remote if present.
3. **Branch safety** — `git::branch_safety()` in `crates/wsp-core/src/git.rs` evaluates the workspace branch against the default branch (`origin/main`). Returns one of four variants, checked in order:

| `BranchSafety` | Meaning | `wsp rm` behavior |
|---|---|---|
| `Merged` | Branch is ancestor of target (regular merge) | Safe, silent removal |
| `SquashMerged` | Tree matches what a squash-merge would produce, or file contents match (`is_content_merged`) | Safe, silent removal |
| `PushedToRemote` | `origin/<branch>` exists but branch is not merged | **Blocked** — requires `--force` |
| `Unmerged` | Branch only exists locally, never pushed | **Blocked** — requires `--force` |

`PushedToRemote` blocks removal to match `git branch -d` semantics: unmerged means unmerged, regardless of whether it's pushed. `--force` is the escape hatch.

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
- `wsp recover` — lists recoverable workspaces, `wsp recover <name>` restores one.
- `gc::maybe_run()` runs after every command (at most once per hour), purging entries older than `gc.retention-days` (default 7, config key `gc.retention-days`).

The gc dir lives alongside mirrors in the XDG data directory (`~/.local/share/wsp/gc/`). `gc::move_dir` uses `fs::rename` when possible, falling back to recursive copy + delete for cross-filesystem moves (EXDEV). GC metadata (`.wsp-gc.yaml`) is written inside the workspace dir before the move.

`workspace::remove(paths, name, force)` — always moves to gc. Tests that previously passed `permanent: true` to bypass gc internals now use the gc path directly.
