# What's New

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
$ wsp config set advice.branchPrefix false   # suppress one hint
$ wsp config set hints false                  # suppress all hints
```

### Per-repo setup commands

Repos can now declare post-clone setup commands (e.g. `npm install`,
`make setup`) in `.wsp.yaml`. When someone creates a workspace
containing your repo, wsp prompts them to approve and run your setup
commands. Commands are hash-verified so changes require re-approval.

```
$ wsp init                          # scaffold .wsp.yaml in current repo
$ wsp repo setup-commands add       # add a command interactively
```

### Automatic branch tracking

`wsp new` and `wsp repo add` now detect when the computed branch name
matches an existing remote branch and automatically set up tracking.
Previously this required `-b <branch>` explicitly. The `-b` flag is
still available when you want to target a different remote branch.

```
$ wsp new my-feature                # auto-tracks origin/my-feature if it exists
$ wsp new my-feature -b main        # explicit: track main instead
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
is now the only path; use `wsp gc` to purge). The `--force` flag on
`wsp repo setup` has been removed. The `wsp-core` public API surface
has been narrowed.
