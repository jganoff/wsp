# ws

Multi-repo workspace manager. Create isolated, branch-per-feature workspaces
across multiple repositories in seconds.

## Why ws?

Working across multiple repos on one feature usually means manually cloning,
branching, and keeping track of which repos are on which branch. `ws` handles
all of that:

- **Instant** — local clones from bare mirrors via hardlinks, no network
- **Isolated** — every workspace is a set of fully independent git clones
- **One command** to create a workspace with consistent branches across repos
- **Safe cleanup** — detects uncommitted work, unmerged and squash-merged branches
- **Context repos** — pin dependencies to a ref without branching them

## Quick start

### Install

Download a binary from the [latest release](https://github.com/jganoff/ws/releases/latest):

| Platform | Archive |
|----------|---------|
| macOS (Apple Silicon) | `ws-aarch64-apple-darwin.tar.xz` |
| macOS (Intel) | `ws-x86_64-apple-darwin.tar.xz` |
| Linux (x86_64) | `ws-x86_64-unknown-linux-gnu.tar.xz` |
| Linux (ARM64) | `ws-aarch64-unknown-linux-gnu.tar.xz` |
| Windows | `ws-x86_64-pc-windows-msvc.zip` |

Or build from source:

```
cargo install --git https://github.com/jganoff/ws.git
```

### Shell integration

Add to your shell rc file:

```bash
# zsh (~/.zshrc)
eval "$(ws setup completion zsh)"

# bash (~/.bashrc)
eval "$(ws setup completion bash)"

# fish (~/.config/fish/config.fish)
ws setup completion fish | source
```

This gives you tab completion and auto-cd into workspaces after `ws new`.

### Register repos

Register your repos once. This creates a bare mirror for fast cloning:

```
$ ws setup repo add git@github.com:acme/api-gateway.git
$ ws setup repo add git@github.com:acme/user-service.git
```

### Create a workspace

```
$ ws new add-billing api-gateway user-service
Creating workspace "add-billing" with 2 repos...
Workspace created: ~/dev/workspaces/add-billing
```

Every repo gets a local clone on the `add-billing` branch.

Need a repo for reference but won't change it? Pin it with `@ref`:

```
$ ws new add-billing api-gateway user-service@main proto@v1.0
```

- `api-gateway` — checked out on the `add-billing` branch
- `user-service@main` — pinned to `main`, no workspace branch
- `proto@v1.0` — pinned to tag `v1.0`

## Day-to-day

```
$ ws cd add-billing          # jump into the workspace
$ ws st                      # status across all repos
$ ws diff                    # diff across all repos
$ ws exec add-billing -- make test   # run a command in every repo
$ ws rm add-billing          # clean up when done
```

Status shows branches, commits ahead, and changed files at a glance:

```
$ ws st
Workspace: add-billing  Branch: add-billing

Repository    Branch        Status
api-gateway   add-billing   1 ahead, 2 files changed
user-service  main          clean
proto         v1.0          clean
```

`ws rm` is safe by default — it blocks if any repo has uncommitted work or
unmerged branches (including squash-merged PRs). Use `--force` to override.

## Commands

| Command | Description |
|---------|-------------|
| `ws new <name> [repos...] [-g group]` | Create a workspace |
| `ws ls` | List workspaces |
| `ws st [workspace]` | Git status across repos |
| `ws diff [workspace] [-- args]` | Git diff across repos |
| `ws rm [workspace] [-f]` | Remove a workspace |
| `ws cd <workspace>` | Change directory into a workspace |
| `ws exec <workspace> -- <cmd>` | Run a command in each repo |
| `ws repo add [repos...] [-g group]` | Add repos to current workspace |
| `ws repo rm <repos...> [-f]` | Remove repos from current workspace |
| `ws repo fetch [--all] [--prune]` | Fetch updates (parallel) |
| `ws setup repo add/list/remove` | Manage registered repositories |
| `ws setup group new/list/show/update/delete` | Manage repo groups |
| `ws setup config list/get/set/unset` | Manage configuration |
| `ws setup completion zsh\|bash\|fish` | Shell integration |

All commands support `--json` for structured output.

See [docs/usage.md](docs/usage.md) for the full reference.

## Configuration

**Branch prefix** — prepend your name to all workspace branches:

```
$ ws setup config set branch-prefix myname
# ws new fix-billing → creates branch myname/fix-billing
```

**Groups** — name a set of repos for quick workspace creation:

```
$ ws setup group new backend api-gateway user-service
$ ws new fix-billing -g backend
```

**Go workspaces** — `ws` auto-generates `go.work` when it detects `go.mod`
files. Disable with `ws setup config set language-integrations.go false`.

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
    api-gateway/             local clone (branch: add-billing)
    user-service/            local clone (ref: main)
```

Each repo is registered once as a bare mirror. Workspaces are directories of
local clones (via `git clone --local` hardlinks) that share a branch name.
Each clone has two remotes: `origin` (real upstream for push/pull) and
`ws-mirror` (local mirror for fast fetch). Context repos (with `@ref`) check
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
