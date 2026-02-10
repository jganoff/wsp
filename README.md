# ws

Multi-repo workspace manager. Create isolated, branch-per-feature workspaces
across multiple repositories in seconds.

`ws` uses bare git mirrors and worktrees under the hood. Registering a repo
clones it once. Every workspace after that is instant -- no network, no
conflicts, no juggling branches between features.

## Why

Working across multiple repos on one feature usually means manually cloning,
branching, and keeping track of which repos are on which branch. `ws` handles
all of that:

- **One command** to create a workspace with consistent branches across repos
- **Instant** -- worktrees from bare mirrors, no re-cloning
- **Isolated** -- every workspace has its own working tree and branch
- **Context repos** -- pin dependencies to a ref without branching them

## Install

```
cargo install --git https://github.com/jganoff/ws.git
```

## Quick start

### Set up

Register your repos once:

```
$ ws repo add git@github.com:acme/api-gateway.git
Cloning git@github.com:acme/api-gateway.git...
Registered github.com/acme/api-gateway

$ ws repo add git@github.com:acme/user-service.git
```

Create a workspace. Every repo gets a worktree on the same branch:

```
$ ws new add-billing api-gateway user-service
Creating workspace "add-billing" with 2 repos...
Workspace created: /Users/you/dev/workspaces/add-billing
```

Need a repo for reference but won't change it? Pin it with `@ref`:

```
$ ws new add-billing api-gateway user-service@main proto@v1.0
```

- `api-gateway` -- checked out on the `add-billing` branch
- `user-service@main` -- pinned to `main`, no workspace branch
- `proto@v1.0` -- pinned to tag `v1.0`

### Work

A workspace is just a directory of git worktrees. Use your normal git workflow
-- commit, push, open PRs, whatever you usually do:

```
$ cd ~/dev/workspaces/add-billing/api-gateway
$ git branch
* add-billing

# hack hack hack
$ git add -A && git commit -m "add billing endpoint"
$ git push -u origin add-billing
```

Check status across all repos at once:

```
$ ws status add-billing
Workspace: add-billing  Branch: add-billing

[api-gateway  ]  (add-billing)  1 ahead  2 files changed
[user-service ]  (main       )  clean
[proto        ]  (v1.0       )  clean
```

### Clean up

Once your branches are merged, remove the workspace:

```
$ ws remove add-billing
Removing workspace "add-billing"...
Workspace "add-billing" removed.
```

This removes the worktrees, deletes the merged branches from your local
mirrors, and cleans up the workspace directory. Your mirrors stay intact for
the next workspace.

`ws remove` is safe by default:

- **Pending changes** -- if any repo has uncommitted work, `ws` refuses to
  remove and tells you which repos are dirty.
- **Unmerged branches** -- if a workspace branch hasn't been merged yet, `ws`
  fetches from the remote to double-check, then blocks removal and lists the
  unmerged repos.
- **`--force`** -- overrides both checks if you really want to delete
  everything.

Nothing gets silently lost.

## Shell integration

Add to your `.zshrc`:

```zsh
eval "$(ws completion zsh)"
```

This gives you tab completion for workspace names, repo shortnames, and group
names, plus auto-cd into workspaces after `ws new`.

## How it works

```
~/.local/share/ws/
  config.yaml
  mirrors/
    github.com/acme/
      api-gateway.git/       bare mirror (one network clone)
      user-service.git/      bare mirror

~/dev/workspaces/
  add-billing/
    .ws.yaml                 workspace metadata
    api-gateway/             worktree (branch: add-billing)
    user-service/            worktree (ref: main)
```

Each repo is cloned once as a bare mirror. Workspaces are directories of
worktrees that share a branch name. Context repos (with `@ref`) check out at
the pinned ref without creating the workspace branch.

## Documentation

See [docs/usage.md](docs/usage.md) for the full command reference,
configuration options, groups, shortname resolution, and more.

## Development

Requires [Rust](https://www.rust-lang.org/tools/install) (stable).

```
cargo build            # build
cargo test             # run tests
cargo clippy           # lint
cargo fmt              # format
```

## License

[MIT](LICENSE)
