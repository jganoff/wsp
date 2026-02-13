# wsp

Multi-repo workspace manager. Create isolated, branch-per-feature workspaces
across multiple repositories in seconds.

## Why wsp?

Working across multiple repos on one feature usually means manually cloning,
branching, and keeping track of which repos are on which branch. `wsp` handles
all of that:

- **Instant** — local clones from bare mirrors via hardlinks, no network
- **Isolated** — every workspace is a set of fully independent git clones
- **One command** — create a workspace with consistent branches across repos
- **Safe cleanup** — detects uncommitted work, unmerged and squash-merged branches
- **Context repos** — pin dependencies to a ref without branching them

## Quick start

### Install

```
brew install jganoff/tap/wsp
```

Or download a binary from the [latest release](https://github.com/jganoff/wsp/releases/latest), or build from source:

```
cargo install --git https://github.com/jganoff/wsp.git
```

### Shell integration

Add to your shell rc file:

```bash
# zsh (~/.zshrc)
eval "$(wsp setup completion zsh)"

# bash (~/.bashrc)
eval "$(wsp setup completion bash)"

# fish (~/.config/fish/config.fish)
wsp setup completion fish | source
```

This gives you tab completion and auto-cd into workspaces after `wsp new`.

### Register repos

Register your repos once. This creates a bare mirror for fast cloning:

```
$ wsp setup repo add git@github.com:acme/api-gateway.git
$ wsp setup repo add git@github.com:acme/user-service.git
```

### Create a workspace

```
$ wsp new add-billing api-gateway user-service
Creating workspace "add-billing" with 2 repos...
Workspace created: ~/dev/workspaces/add-billing
```

Every repo gets a local clone on the `add-billing` branch.

Need a repo for reference but won't change it? Pin it with `@ref`:

```
$ wsp new add-billing api-gateway user-service@main proto@v1.0
```

- `api-gateway` — checked out on the `add-billing` branch
- `user-service@main` — pinned to `main`, no workspace branch
- `proto@v1.0` — pinned to tag `v1.0`

## Day-to-day

```
$ wsp cd add-billing          # jump into the workspace
$ wsp st                      # status across all repos
$ wsp diff                    # diff across all repos
$ wsp exec add-billing -- make test   # run a command in every repo
$ wsp rm add-billing          # clean up when done
```

Status shows branches, commits ahead, and changed files at a glance:

```
$ wsp st
Workspace: add-billing  Branch: add-billing

Repository    Branch        Status
api-gateway   add-billing   1 ahead, 2 files changed
user-service  main          clean
proto         v1.0          clean
```

`wsp rm` is safe by default — it blocks if any repo has uncommitted work or
unmerged branches (including squash-merged PRs). Use `--force` to override.

## Commands

| Command | Description |
|---------|-------------|
| `wsp new <name> [repos...] [-g group]` | Create a workspace |
| `wsp ls` | List workspaces |
| `wsp st [workspace]` | Git status across repos |
| `wsp diff [workspace] [-- args]` | Git diff across repos |
| `wsp rm [workspace] [-f]` | Remove a workspace |
| `wsp cd <workspace>` | Change directory into a workspace |
| `wsp exec <workspace> -- <cmd>` | Run a command in each repo |
| `wsp repo add [repos...] [-g group]` | Add repos to current workspace |
| `wsp repo rm <repos...> [-f]` | Remove repos from current workspace |
| `wsp repo fetch [--all] [--prune]` | Fetch updates (parallel) |
| `wsp setup repo add/list/remove` | Manage registered repositories |
| `wsp setup group new/list/show/update/delete` | Manage repo groups |
| `wsp setup config list/get/set/unset` | Manage configuration |
| `wsp setup completion zsh\|bash\|fish` | Shell integration |

All commands support `--json` for structured output.

See [docs/usage.md](docs/usage.md) for the full reference.

## Configuration

**Branch prefix** — prepend your name to all workspace branches:

```
$ wsp setup config set branch-prefix myname
# wsp new fix-billing → creates branch myname/fix-billing
```

**Groups** — name a set of repos for quick workspace creation:

```
$ wsp setup group new backend api-gateway user-service
$ wsp new fix-billing -g backend
```

**Go workspaces** — `wsp` auto-generates `go.work` when it detects `go.mod`
files. Disable with `wsp setup config set language-integrations.go false`.

## How it works

```
~/.local/share/wsp/
  config.yaml
  mirrors/
    github.com/acme/
      api-gateway.git/       bare mirror (one network clone)
      user-service.git/      bare mirror

~/dev/workspaces/
  add-billing/
    .wsp.yaml                 workspace metadata
    api-gateway/             local clone (branch: add-billing)
    user-service/            local clone (ref: main)
```

Each repo is registered once as a bare mirror. Workspaces are directories of
local clones (via `git clone --local` hardlinks) that share a branch name.
Each clone has two remotes: `origin` (real upstream for push/pull) and
`wsp-mirror` (local mirror for fast fetch). Context repos (with `@ref`) check
out at the pinned ref without creating the workspace branch.

## Development

Requires [Rust](https://www.rust-lang.org/tools/install) (stable) and
[just](https://github.com/casey/just).

```
just          # check (fmt + clippy)
just build    # build release binary
just test     # run all tests
just ci       # full CI pipeline
just fix      # auto-fix formatting and lint
```

## License

[MIT](LICENSE)
