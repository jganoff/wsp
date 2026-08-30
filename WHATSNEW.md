# What's New

## [0.19.0] - 2026-08-29

### Breaking: `wsp recover` no longer lists

`wsp recover` with no arguments used to list what could be restored, and `wsp
recover show <name>` printed one workspace's details. Both are gone. `wsp ls
--removed` replaces them:

```
wsp recover              ->  wsp ls --removed
wsp recover show <name>  ->  wsp ls --removed --size
```

`wsp recover <name>` is unchanged, and now works for every name. A workspace
called `ls` or `show` could not be recovered at all before. Bare `wsp recover`
tells you how many workspaces are recoverable and points at `wsp ls
--removed`.

### PowerShell shell integration

`wsp completion powershell` sets up tab completion and the `wsp cd` wrapper.

```
wsp completion powershell
```

### Removed workspaces show up in `wsp ls`

`wsp ls --removed` lists what you removed but can still restore, soonest to
expire first, with their repos and how long is left. `-t`, `-U` and `-r` sort
it.

```
wsp ls --removed
```

Plain `wsp ls` now ends with a line counting what is recoverable, and names the
workspace when it is within a day of being purged for good.

### `wsp ls --size` shows disk usage

How much disk a workspace is using, for the ones you removed as well as the
ones you still have.

```
wsp ls --size
wsp ls --removed --size
```

### wsp tells you when git data is being duplicated

If your workspaces and wsp's data are on different filesystems, each workspace
keeps its own full copy of every repo's history instead of sharing one, and
removing a workspace stops being a single atomic step. `wsp new` warns when it
happens and `wsp doctor` checks for it, both naming the directories to move.

```
wsp doctor
```

### `wsp create`

`create` is now an alias for `new`.

```
wsp create my-feature github.com/acme/api
```

### Fixes

Shell integration (these need `eval "$(wsp completion <shell>)"` in your
shell startup), and all of them go back to v0.14.0:

- `wsp new` and `wsp create` now always change into the new workspace. Before,
  that only worked when you put the name first: `wsp new --empty my-ws` left
  you where you were, and on PowerShell `wsp new -w other my-ws` could drop
  you into the wrong workspace entirely.
- `wsp rm`, `wsp rename` and `wsp repo rm` now work from inside the workspace
  or repo you are acting on. On Windows they failed outright.
- `wsp rm` and `wsp rename` no longer break `cd -`. It takes you back where
  you came from, not to your workspaces directory.
- `wsp rename` keeps you in the equivalent subdirectory of the renamed
  workspace instead of moving you to its root.
- `wsp cd <workspace> --json` no longer fails.

Everything else:

- `wsp new` no longer leaves a repo with no commits when its default branch is
  not `main`. On some versions of git the workspace branch was created from
  nothing, so every file showed as staged and `wsp rm` refused with "unsaved
  work" that did not exist.
- `wsp new` and `wsp repo add` work with repos whose default branch is not
  `main`, including branch names containing slashes.
- `wsp new` no longer fails when a workspace and its mirror are on different
  filesystems and hardlinks are unavailable; it copies the git objects instead.
- `wsp new` no longer creates an empty branch when a populated mirror's default
  branch is temporarily unreadable.
- `wsp repo add` now fetches before cloning. Adding a repo you had added before
  gave you whatever was last fetched, which could be weeks old. It also made a
  branch you had just pushed look missing, so the repo quietly started a new
  branch instead of tracking yours. Pass `--no-fetch` to skip the fetch.
- Per-repo setup commands now run on Windows. They were invoked through `sh`,
  which is not there by default, so they failed.
- If Windows will not let `wsp` create symlinks (Developer Mode off, and not
  running elevated), it keeps going instead of failing. The `CLAUDE.md` link is
  skipped, and `wsp doctor` tells you what to turn on.
- `wsp ls`, `wsp st` and other read-only commands no longer delete expired
  recoverable workspaces as a side effect. Cleanup only happens after a command
  that changes something, and says what it removed.
- `wsp ls` no longer hides the recoverable-workspace notice when you have no
  active workspaces left, which is exactly when you need it.
- `wsp ls` keeps every workspace row aligned when descriptions contain line
  breaks, and wraps long descriptions to fit your terminal.
- `wsp rm` tells you the date recovery stops working, instead of a duration you
  have to count from, and no longer claims a workspace is recoverable when it
  had no metadata and was deleted outright.
- `wsp rm` no longer blocks on linked worktrees that git has already marked
  prunable. Live worktrees still block, since removing the workspace would
  orphan them.
- `wsp st` and `wsp rm` now say when they are fetching pull requests. With
  `pr.source = github` set, a large workspace looked like a hung terminal.
- `wsp exec`, `wsp repo fetch` and `wsp sync` no longer crash when you pipe them
  into something that stops reading early, such as `head` or `grep -q`. They
  stop quietly, and `wsp exec` no longer reports the interrupted command as a
  failure.
- `wsp doctor --fix` no longer warns that a repo's origin URL differs from the
  registry right after registering that repo itself.

Full commit log: https://github.com/jganoff/wsp/releases/tag/v0.19.0

## [0.18.0] - 2026-05-29

`wsp` now works properly on Windows: a PowerShell one-liner installs it,
hints and error messages render correctly instead of showing `◙` characters,
and `wsp registry add --from` defaults to HTTPS so users without SSH
configured don't hit silent clone failures.

### Windows support

Install `wsp` on Windows with a single PowerShell command:

```
irm https://github.com/jganoff/wsp/releases/latest/download/wsp-installer.ps1 | iex
```

Hints and multi-line output that previously appeared as `◙` in PowerShell 5.1
now display correctly.

### Registry HTTPS default

`wsp registry add --from` now clones via HTTPS by default instead of SSH.
SSH is not configured by default on Windows, causing silent clone failures
for users who only have HTTPS auth.

Set your preferred protocol once globally:

```
wsp config set clone.protocol ssh
```

Or override per invocation with `--ssh` or `--https`.

### Fixes

- `wsp rm` and `wsp repo rm` now check the currently checked-out branch for
  unmerged work, not just the workspace branch. Previously, local commits on
  a different branch were silently discarded on removal.

## [0.17.1] - 2026-04-16

`wsp whatsnew --all` (or `-a`) now dumps the full release history,
newest first. Prose release notes are used where available, with a
fallback to the commit-level changelog for versions that predate
prose notes. Default behavior is unchanged: `wsp whatsnew` still
shows only the current version, matching the upgrade hint.

```
wsp whatsnew --all
```

## [0.17.0] - 2026-04-16

`wsp st` runs per-repo git queries in parallel for a measurable speedup,
`wsp describe` accepts freeform text after `--` without quoting, and
`wsp rm` consolidates its safety checks and PR warning into a single
confirmation prompt.

### Faster status

`wsp st` runs each repo's git queries (branch, upstream, ahead/behind,
changed files) concurrently instead of sequentially. Larger workspaces
see the biggest improvement.

```
wsp st
```

### Describe without quoting

`wsp describe` now supports `--` to pass everything after it as the
description. This removes shell-quoting friction when the text contains
flags, UUIDs, or command fragments.

```
wsp describe -- claude --resume abc123
wsp describe my-ws -- fix auth middleware
```

### Consolidated rm confirmation

`wsp rm` evaluates all safety checks up front and folds the
pushed-but-unmerged branch warning into the open-PR prompt. Hard
blockers (uncommitted changes, linked worktrees, local unmerged
branches) still error immediately, but you only answer one prompt for
the remaining warnings.

```
wsp rm my-workspace
```

### Security

The git config denylist now covers `alias.`, `browser.`,
`interactive.difffilter`, `man.`, `pager.`, and the full `sendemail.`
prefix. `.wsp.yaml` files are validated against the denylist at load
time, matching the existing check on template files. Approvals storage
gained file locking and atomic writes to match the pattern used
elsewhere in the codebase.

## [0.16.0] - 2026-04-12

`wsp st` gets a redesigned output with a dedicated PR column and
aligned metadata header, and `wsp whatsnew` now renders styled
release notes directly in your terminal.

### Redesigned status output

`wsp st` now shows pull request information in its own column
instead of appending a label to the status field. The PR column
displays the PR number and state, with full details available
in verbose mode. The header metadata block is now column-aligned
with a stable layout that always includes all fields.

```
wsp st
wsp st -v
```

### Styled release notes

`wsp whatsnew` renders release notes with ANSI formatting for
headings, code blocks, and inline code. Notes are written in
prose alongside the auto-generated changelog, giving you a
quick summary of what changed and what to try.

```
wsp whatsnew
```

### Optional workspace positional in describe and rename

`wsp describe` and `wsp rename` now detect the workspace from
your current directory, matching the convention used by other
commands. You can still pass the workspace name explicitly.

```
cd ~/dev/workspaces/my-feature
wsp describe "migrating to stripe v3"
```

### Fixes

- `wsp config set pr.source` now accepts `github` instead of
  the undocumented `gh` value. The old value still works but
  prints a deprecation warning.

## [0.15.0] - 2026-04-04

v0.15.0 adds PR awareness to workspace status, a contextual hint system
that teaches you features as you work, and per-repo setup commands that
run automatically after cloning. Branch tracking is now automatic when
your workspace name matches a remote branch.

### PR awareness in status and removal

`wsp st` now shows open pull requests for each repo, and `wsp rm` warns
when a workspace has unmerged PRs. No configuration needed; works
automatically when `gh` is installed and authenticated.

```
$ wsp st
```

### Contextual hints

wsp now shows git-style hints after commands when it detects you might
benefit from a feature you haven't tried. For example, after `wsp new`
without a branch prefix configured, you'll see a suggestion to set one.
Hints appear at most once per day and are individually suppressible.

```
$ wsp config set advice.branchPrefix false
$ wsp config set hints false
```

### Per-repo setup commands

Repos can now declare post-clone setup commands (e.g. `npm install`,
`make setup`) in `.wsp.yaml`. When someone creates a workspace
containing your repo, wsp prompts them to approve and run your setup
commands. Commands are hash-verified so changes require re-approval.

```
$ wsp init
$ wsp repo setup-commands add
```

### Automatic branch tracking

`wsp new` and `wsp repo add` now detect when the computed branch name
matches an existing remote branch and automatically set up tracking.
Previously this required `-b <branch>` explicitly. The `-b` flag is
still available when you want to target a different remote branch.

```
$ wsp new my-feature
$ wsp new my-feature -b main
```

### Empty workspaces

`wsp new --empty` creates a workspace directory and metadata without
cloning any repos. Useful when you want to incrementally add repos.

```
$ wsp new my-feature --empty
$ wsp repo add api-server web-client
```

### Upgrade notice

After upgrading wsp, the first command you run shows a one-time hint
pointing you to `wsp whatsnew`.

### Fixes

- `wsp sync` skips repos on the wrong branch instead of erroring, so
  partial syncs no longer block the rest of the workspace
- Credentials are stripped from HTTPS URLs before persisting to config
- `wsp rm` handles partial workspaces (missing `.wsp.yaml`) gracefully
- Shell completion no longer breaks `eval $(wsp completion zsh)` without
  quotes
- Workspace detection skips per-repo `.wsp.yaml` files, avoiding false
  positives when running from inside a repo subdirectory
- `wsp recover` now has shell completion for workspace names

### Internal

The `--permanent` flag on `wsp rm` has been removed (deferred deletion
is now the only path; expired entries are purged automatically based
on `gc.retention-days`). The `--force` flag on `wsp repo setup` has
been removed. The `wsp-core` public API surface has been narrowed.
