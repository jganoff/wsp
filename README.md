# ws

Spin up isolated, ready-to-code workspaces across multiple repositories in
seconds.

`ws` uses git worktrees backed by bare mirrors, so creating a workspace is
instant (no network clone) and every workspace gets its own working tree with its
own branch. No conflicts, no juggling state between features.

## Install

```
cargo install --git https://github.com/jganoff/ws.git
```

Or build from source:

```
git clone https://github.com/jganoff/ws.git
cd ws
cargo install --path .
```

## Quick start

### 1. Single repo (minimum viable)

Register a repo and create a workspace:

```
$ ws repo add git@github.com:acme/api-gateway.git
Cloning git@github.com:acme/api-gateway.git...
Registered github.com/acme/api-gateway

$ ws new fix-billing api-gateway
Creating workspace "fix-billing" with 1 repos...
Workspace created: /Users/you/dev/workspaces/fix-billing

$ cd ~/dev/workspaces/fix-billing/api-gateway
$ git branch
* fix-billing
```

That's it. Two commands to get an isolated worktree on a new branch.

### 2. Multiple repos

Register more repos, then create a workspace with all of them:

```
$ ws repo add git@github.com:acme/user-service.git
$ ws repo add git@github.com:acme/web-app.git

$ ws new add-billing api-gateway user-service web-app
Creating workspace "add-billing" with 3 repos...
Workspace created: /Users/you/dev/workspaces/add-billing
```

Check status:

```
$ ws status add-billing
Workspace: add-billing  Branch: add-billing

[api-gateway  ]  (add-billing)  clean
[user-service ]  (add-billing)  clean
[web-app      ]  (add-billing)  clean
```

### 3. Groups

If you always use the same set of repos, save them as a group:

```
$ ws group new backend api-gateway user-service
Created group "backend" with 2 repos

$ ws new add-billing -g backend web-app
Creating workspace "add-billing" with 3 repos...
Workspace created: /Users/you/dev/workspaces/add-billing
```

### 4. Context repos (`@ref`)

Some repos are just for reference -- you won't change them. Pin them to a
branch or tag with `@ref`:

```
$ ws new add-billing api-gateway user-service@main proto@v1.0
Creating workspace "add-billing" with 3 repos...
Workspace created: /Users/you/dev/workspaces/add-billing
```

- `api-gateway` -- checked out on the `add-billing` branch (active)
- `user-service@main` -- checked out at `main` (context, no workspace branch)
- `proto@v1.0` -- checked out at tag `v1.0` (context, detached HEAD)

```
$ ws status add-billing
Workspace: add-billing  Branch: add-billing

[api-gateway  ]  (add-billing)  3 ahead  2 files changed
[user-service ]  (main       )  clean
[proto        ]  (v1.0       )  clean
```

### 5. Branch prefix

Set a global branch prefix so every workspace branch is created under your
namespace:

```
$ ws config set branch-prefix jganoff
branch-prefix = jganoff

$ ws new fix-billing api-gateway
Creating workspace "fix-billing" (branch: jganoff/fix-billing) with 1 repos...
Workspace created: /Users/you/dev/workspaces/fix-billing

$ cd ~/dev/workspaces/fix-billing/api-gateway
$ git branch
* jganoff/fix-billing
```

The workspace directory name stays `fix-billing` -- only the git branch gets
the prefix. To check or remove the prefix:

```
$ ws config get branch-prefix
jganoff

$ ws config unset branch-prefix
branch-prefix unset
```

### 6. Day-to-day commands

```
$ ws add proto@v1.0                         # add a repo to current workspace
$ ws exec add-billing -- make test          # run command across all repos
$ ws remove add-billing --delete-branches   # clean up
```

## Shell integration

Add this to your `.zshrc`:

```zsh
eval "$(ws completion zsh)"
```

This gives you:

- **Tab completion** for workspace names, repo shortnames, and group names
- **Auto-cd** into the workspace directory after `ws new`
- **Auto-cd out** of a workspace directory before `ws remove` if you're inside it
- All other subcommands pass through to the binary unchanged

## How it works

`ws` keeps a single bare mirror of each repository you register. Mirrors live in
a shared object store so there is only one network clone per repo, no matter how
many workspaces reference it. When you create a workspace, `ws` adds a git
worktree from each mirror for the workspace branch and groups them into a single
directory. A workspace is just a folder of worktrees that all share the same
branch name.

Context repos (those with `@ref`) are checked out at the specified ref without
creating the workspace branch.

```
~/.local/share/ws/
  config.yaml
  mirrors/
    github.com/
      acme/
        api-gateway.git/        bare mirror
        user-service.git/       bare mirror
        proto.git/              bare mirror

~/dev/workspaces/
  add-billing/
    .ws.yaml                    workspace metadata
    api-gateway/                worktree (branch: add-billing)
    user-service/               worktree (ref: main)
    proto/                      worktree (ref: v1.0)
```

## Command reference

### Repos

#### `ws repo add <url>`

Register a repository and create its bare mirror.

```
$ ws repo add git@github.com:acme/api-gateway.git
Cloning git@github.com:acme/api-gateway.git...
Registered github.com/acme/api-gateway
```

#### `ws repo list`

List all registered repositories.

```
$ ws repo list
  github.com/acme/api-gateway [api-gateway]  (git@github.com:acme/api-gateway.git)
  github.com/acme/user-service [user-service]  (git@github.com:acme/user-service.git)
```

Shows identity, shortname (in brackets if different from identity), and URL.

#### `ws repo remove <name>`

Remove a repository and delete its bare mirror. Accepts a shortname.

```
$ ws repo remove api-gateway
Removing mirror for github.com/acme/api-gateway...
Removed github.com/acme/api-gateway
```

#### `ws repo fetch [name]`

Fetch updates for one or all mirrors. With no arguments, fetches all repos.

| Flag    | Description             |
|---------|-------------------------|
| `--all` | Fetch all registered repos |

```
$ ws repo fetch api-gateway
Fetching github.com/acme/api-gateway...

$ ws repo fetch
Fetching github.com/acme/api-gateway...
Fetching github.com/acme/user-service...
```

### Groups

#### `ws group new <name> <repos...>`

Create a named group of repositories.

```
$ ws group new backend api-gateway user-service
Created group "backend" with 2 repos
```

#### `ws group list`

List all groups.

```
$ ws group list
  backend (2 repos)
  frontend (1 repos)
```

#### `ws group show <name>`

Show the repos in a group.

```
$ ws group show backend
Group "backend":
  github.com/acme/api-gateway
  github.com/acme/user-service
```

#### `ws group delete <name>`

Delete a group. Does not affect the repos themselves.

```
$ ws group delete backend
Deleted group "backend"
```

### Config

#### `ws config get <key>`

Get a config value.

```
$ ws config get branch-prefix
jganoff
```

#### `ws config set <key> <value>`

Set a config value.

```
$ ws config set branch-prefix jganoff
branch-prefix = jganoff
```

#### `ws config unset <key>`

Unset a config value.

```
$ ws config unset branch-prefix
branch-prefix unset
```

**Available keys:**

| Key             | Description                                         |
|-----------------|-----------------------------------------------------|
| `branch-prefix` | Prefix prepended to workspace branch names (`prefix/name`) |

### Workspaces

#### `ws new <workspace> [repos...] [-g group]`

Create a workspace. Each listed repo gets a worktree checked out to a branch
matching the workspace name. Repos with `@ref` are checked out at that ref as
context repos (no workspace branch created).

| Flag          | Description            |
|---------------|------------------------|
| `-g, --group` | Include repos from a group |

```
$ ws new add-billing -g backend web-app proto@v1.0
Creating workspace "add-billing" with 4 repos...
Workspace created: /Users/you/dev/workspaces/add-billing
```

#### `ws add [repos...] [-g group]`

Add repos to the current workspace. Must be run from inside a workspace
directory (or a subdirectory of one). Supports `@ref` syntax for context repos.

| Flag          | Description            |
|---------------|------------------------|
| `-g, --group` | Include repos from a group |

```
$ cd ~/dev/workspaces/add-billing
$ ws add proto@v1.0
Adding 1 repos to workspace...
Done.
```

#### `ws list`

List all workspaces.

```
$ ws list
  add-billing  branch:add-billing  repos:3  /Users/you/dev/workspaces/add-billing
  fix-auth     branch:fix-auth     repos:2  /Users/you/dev/workspaces/fix-auth
```

#### `ws status [workspace]`

Show git branch and working tree status for every repo in a workspace. If no
workspace name is given, detects the current workspace from the working
directory.

```
$ ws status add-billing
Workspace: add-billing  Branch: add-billing

[api-gateway  ]  (add-billing)  3 ahead  2 files changed
[user-service ]  (main       )  clean
[proto        ]  (v1.0       )  clean
```

#### `ws remove <workspace> [--delete-branches]`

Remove a workspace and its worktrees. `--delete-branches` only deletes the
workspace branch for active repos; context repos are unaffected.

| Flag                | Description                            |
|---------------------|----------------------------------------|
| `--delete-branches` | Also delete workspace branches from mirrors |

```
$ ws remove add-billing --delete-branches
Removing workspace "add-billing"...
Workspace "add-billing" removed.
```

#### `ws exec <workspace> -- <command...>`

Run a command in every repo directory of a workspace.

```
$ ws exec add-billing -- make test
==> [api-gateway] make test
ok

==> [user-service] make test
ok

==> [proto] make test
ok
```

## `.ws.yaml` format

```yaml
name: add-billing
branch: add-billing
repos:
  github.com/acme/api-gateway:
  github.com/acme/user-service:
    ref: main
  github.com/acme/proto:
    ref: v1.0
created: 2025-06-15T11:00:00Z
```

Active repos have no value (nil entry). Context repos have a `ref` field
specifying the pinned branch or tag.

## Shortname resolution

Repos are identified by their full identity (`host/owner/repo`). When names are
unambiguous, `ws` lets you use shorter names.

| Registered repos                                            | Input           | Resolves to                  |
|-------------------------------------------------------------|-----------------|------------------------------|
| `github.com/acme/api-gateway`, `github.com/acme/web-app`   | `api-gateway`   | `github.com/acme/api-gateway` |
| `github.com/acme/api-gateway`, `github.com/acme/web-app`   | `web-app`       | `github.com/acme/web-app`     |
| `github.com/acme/utils`, `github.com/other/utils`          | `utils`         | error: ambiguous             |
| `github.com/acme/utils`, `github.com/other/utils`          | `acme/utils`    | `github.com/acme/utils`       |
| `github.com/acme/utils`, `github.com/other/utils`          | `other/utils`   | `github.com/other/utils`      |

Resolution walks the identity segments from right to left and picks the shortest
suffix that uniquely matches exactly one registered repo. If a suffix matches
multiple repos, you need to provide more segments.

## Workspace detection

`ws add` and `ws status` (when called without arguments) detect the current
workspace by walking up from the working directory until they find a `.ws.yaml`
file. This means you can run these commands from any subdirectory inside a
workspace:

```
$ cd ~/dev/workspaces/add-billing/api-gateway/src
$ ws status
Workspace: add-billing  Branch: add-billing
...
```

## Configuration

### Data directory

All `ws` data is stored under `~/.local/share/ws/`. If `XDG_DATA_HOME` is set,
`ws` uses `$XDG_DATA_HOME/ws/` instead.

```
~/.local/share/ws/
  config.yaml           registered repos and groups
  mirrors/              bare git clones
```

### Workspaces directory

Workspaces are created under `~/dev/workspaces/`.

### config.yaml

```yaml
branch_prefix: jganoff

repos:
  github.com/acme/api-gateway:
    url: git@github.com:acme/api-gateway.git
    added: 2025-06-15T10:30:00Z
  github.com/acme/user-service:
    url: git@github.com:acme/user-service.git
    added: 2025-06-15T10:31:00Z

groups:
  backend:
    repos:
      - github.com/acme/api-gateway
      - github.com/acme/user-service
```

## Development

Requires [Rust](https://www.rust-lang.org/tools/install) (stable) with the
`clippy` and `rustfmt` components:

```
rustup component add clippy rustfmt
```

### Building

```
cargo build --release    # optimized binary at target/release/ws
cargo install --path .   # install to ~/.cargo/bin/ws
```

### Testing

```
cargo test -- --test-threads=1
```

Tests must run single-threaded (`--test-threads=1`) because some tests mutate
process environment variables.

### Linting and formatting

```
cargo clippy -- -D warnings   # lint (warnings are errors)
cargo fmt                      # auto-format
cargo fmt --check              # check formatting (CI)
```

### CI

CI runs on every push to `main` and on pull requests. It checks formatting,
runs clippy, builds, and runs all tests. See
[`.github/workflows/ci.yml`](.github/workflows/ci.yml).
