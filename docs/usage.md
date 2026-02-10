# Usage

Full command reference and configuration guide for `ws`.

## Repos

### `ws repo add <url>`

Register a repository and create its bare mirror.

```
$ ws repo add git@github.com:acme/api-gateway.git
Cloning git@github.com:acme/api-gateway.git...
Registered github.com/acme/api-gateway
```

### `ws repo list`

List all registered repositories.

```
$ ws repo list
  github.com/acme/api-gateway [api-gateway]  (git@github.com:acme/api-gateway.git)
  github.com/acme/user-service [user-service]  (git@github.com:acme/user-service.git)
```

Shows identity, shortname (in brackets), and URL.

### `ws repo remove <name>`

Remove a repository and delete its bare mirror. Accepts a shortname.

```
$ ws repo remove api-gateway
Removing mirror for github.com/acme/api-gateway...
Removed github.com/acme/api-gateway
```

### `ws repo fetch [name]`

Fetch updates for one or all mirrors. With no arguments, fetches all repos.

```
$ ws repo fetch api-gateway
Fetching github.com/acme/api-gateway...

$ ws repo fetch
Fetching github.com/acme/api-gateway...
Fetching github.com/acme/user-service...
```

| Flag    | Description              |
|---------|--------------------------|
| `--all` | Fetch all registered repos |

## Groups

Save frequently-used sets of repos as groups.

### `ws group new <name> <repos...>`

Create a named group.

```
$ ws group new backend api-gateway user-service
Created group "backend" with 2 repos
```

### `ws group list`

List all groups.

```
$ ws group list
  backend (2 repos)
  frontend (1 repos)
```

### `ws group show <name>`

Show the repos in a group.

```
$ ws group show backend
Group "backend":
  github.com/acme/api-gateway
  github.com/acme/user-service
```

### `ws group delete <name>`

Delete a group. Does not affect the repos themselves.

```
$ ws group delete backend
Deleted group "backend"
```

## Workspaces

### `ws new <workspace> [repos...] [-g group]`

Create a workspace. Each listed repo gets a worktree checked out to a branch
matching the workspace name. Repos with `@ref` are checked out at that ref as
context repos (no workspace branch created).

| Flag          | Description                |
|---------------|----------------------------|
| `-g, --group` | Include repos from a group |

```
$ ws new add-billing -g backend web-app proto@v1.0
Creating workspace "add-billing" with 4 repos...
Workspace created: /Users/you/dev/workspaces/add-billing
```

### `ws add [repos...] [-g group]`

Add repos to the current workspace. Must be run from inside a workspace
directory. Supports `@ref` syntax for context repos.

| Flag          | Description                |
|---------------|----------------------------|
| `-g, --group` | Include repos from a group |

```
$ cd ~/dev/workspaces/add-billing
$ ws add proto@v1.0
Adding 1 repos to workspace...
Done.
```

### `ws list`

List all workspaces.

```
$ ws list
  add-billing  branch:add-billing  repos:3  /Users/you/dev/workspaces/add-billing
  fix-auth     branch:fix-auth     repos:2  /Users/you/dev/workspaces/fix-auth
```

### `ws status [workspace]`

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

### `ws diff [workspace]`

Show `git diff` across all repos in a workspace.

### `ws remove <workspace> [--delete-branches]`

Remove a workspace and its worktrees.

| Flag                | Description                                |
|---------------------|--------------------------------------------|
| `--delete-branches` | Also delete workspace branches from mirrors |

```
$ ws remove add-billing --delete-branches
Removing workspace "add-billing"...
Workspace "add-billing" removed.
```

### `ws exec <workspace> -- <command...>`

Run a command in every repo directory of a workspace.

```
$ ws exec add-billing -- make test
==> [api-gateway] make test
ok

==> [user-service] make test
ok
```

## Context repos (`@ref`)

Some repos are just for reference -- you won't change them. Pin them to a
branch or tag with `@ref`:

```
$ ws new add-billing api-gateway user-service@main proto@v1.0
```

- `api-gateway` -- checked out on the `add-billing` branch (active)
- `user-service@main` -- checked out at `main` (context, no workspace branch)
- `proto@v1.0` -- checked out at tag `v1.0` (context, detached HEAD)

## Branch prefix

Set a global prefix so every workspace branch is created under your namespace:

```
$ ws config set branch-prefix jganoff

$ ws new fix-billing api-gateway
Creating workspace "fix-billing" (branch: jganoff/fix-billing) with 1 repos...

$ cd ~/dev/workspaces/fix-billing/api-gateway
$ git branch
* jganoff/fix-billing
```

The workspace directory name stays `fix-billing` -- only the git branch gets
the prefix.

## Config

### `ws config get <key>`

Get a config value.

### `ws config set <key> <value>`

Set a config value.

### `ws config unset <key>`

Unset a config value.

### `ws config list`

List all config values.

**Available keys:**

| Key             | Description                                           |
|-----------------|-------------------------------------------------------|
| `branch-prefix` | Prefix prepended to workspace branch names (`prefix/name`) |

## Shortname resolution

Repos are identified by their full identity (`host/owner/repo`). When names are
unambiguous, `ws` lets you use shorter names.

| Registered repos                                          | Input         | Resolves to                    |
|-----------------------------------------------------------|---------------|--------------------------------|
| `github.com/acme/api-gateway`, `github.com/acme/web-app` | `api-gateway` | `github.com/acme/api-gateway`  |
| `github.com/acme/utils`, `github.com/other/utils`        | `utils`       | error: ambiguous               |
| `github.com/acme/utils`, `github.com/other/utils`        | `acme/utils`  | `github.com/acme/utils`        |

Resolution walks identity segments right to left and picks the shortest suffix
that uniquely matches one registered repo. If ambiguous, provide more segments.

## Workspace detection

`ws add` and `ws status` (without arguments) detect the current workspace by
walking up from the working directory until they find a `.ws.yaml` file:

```
$ cd ~/dev/workspaces/add-billing/api-gateway/src
$ ws status
Workspace: add-billing  Branch: add-billing
...
```

## Data layout

### Data directory

All `ws` data is stored under `~/.local/share/ws/`. Respects `XDG_DATA_HOME`.

```
~/.local/share/ws/
  config.yaml           registered repos, groups, settings
  mirrors/              bare git clones
```

### Workspaces directory

Workspaces are created under `~/dev/workspaces/`.

### `.ws.yaml` format

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

### `config.yaml` format

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

## Shell integration

Add to your `.zshrc`:

```zsh
eval "$(ws completion zsh)"
```

This provides:

- Tab completion for workspace names, repo shortnames, and group names
- Auto-cd into the workspace directory after `ws new`
- Auto-cd out of a workspace directory before `ws remove` if you're inside it
- All other subcommands pass through to the binary unchanged
