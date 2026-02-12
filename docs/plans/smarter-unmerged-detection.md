# Smarter unmerged branch detection for `ws rm`

## Problem

`ws rm` uses `git merge-base --is-ancestor` to check whether each workspace
branch has been merged into the default branch. This produces false positives in
two common workflows:

### 1. Squash-merged PRs

When a PR is squash-merged on GitHub, the original branch commits are not
ancestors of the resulting merge commit on `main`. `--is-ancestor` returns false
even though the work is fully merged.

### 2. Pushed but not yet merged

A user pushes their branch and opens a PR, then wants to delete the workspace
to keep things tidy (they can recreate it later from the remote branch). The
branch genuinely isn't merged, but the work is safe on the remote. Currently
this requires `--force`, with a message that reads like something is wrong.

## Possible approaches

### Squash-merge detection

- **`git cherry`**: compare patch-ids between the workspace branch and
  `origin/main`. If all commits have matching patch-ids on the target, the
  branch is effectively merged. This works for squash merges that preserve
  content.
- **Tree comparison**: check if `git diff <branch> origin/<default>` produces
  no output (the trees are identical). Only works if no other changes landed
  after the squash merge.
- **GitHub API**: query the PR status for the branch. Most accurate but adds a
  network dependency and GitHub-specific coupling.

### Pushed-but-unmerged UX

- **Check if branch exists on remote**: after fetch, check
  `refs/remotes/origin/<branch>`. If it exists, the work is safely pushed.
  Allow removal without `--force`, or downgrade the error to a softer warning
  that still requires confirmation.
- **Separate flag**: something like `ws rm --pushed` that only blocks on
  unpushed work, not unmerged work. Avoids overloading `--force` which also
  skips dirty-file checks.

## Decision

TBD
