# Usage

Full command reference and configuration guide for `wsp`.

## Setup

### `wsp setup repo add <url>`

Register a repository and create its bare mirror.

```
$ wsp setup repo add git@github.com:acme/api-gateway.git
Cloning git@github.com:acme/api-gateway.git...
Registered github.com/acme/api-gateway
```

### `wsp setup repo list`

List all registered repositories.

```
$ wsp setup repo list
  github.com/acme/api-gateway [api-gateway]  (git@github.com:acme/api-gateway.git)
  github.com/acme/user-service [user-service]  (git@github.com:acme/user-service.git)
```

Shows identity, shortname (in brackets), and URL.

### `wsp setup repo remove <name>`

Remove a repository and delete its bare mirror. Accepts a shortname.

```
$ wsp setup repo remove api-gateway
Removing mirror for github.com/acme/api-gateway...
Removed github.com/acme/api-gateway
```

### Groups

Save frequently-used sets of repos as groups.

### `wsp setup group new <name> <repos...>`

Create a named group.

```
$ wsp setup group new backend api-gateway user-service
Created group "backend" with 2 repos
```

### `wsp setup group update <name> --add <repos...> --remove <repos...>`

Add or remove repos from an existing group. At least one of `--add` or
`--remove` is required. Errors if adding a repo already in the group, or
removing one that isn't.

```
$ wsp setup group update backend --add api-gateway user-service
Updated group "backend": added 2

$ wsp setup group update backend --remove old-service
Updated group "backend": removed 1

$ wsp setup group update backend --add new-svc --remove old-svc
Updated group "backend": added 1, removed 1
```

### `wsp setup group list`

List all groups.

```
$ wsp setup group list
  backend (2 repos)
  frontend (1 repos)
```

### `wsp setup group show <name>`

Show the repos in a group.

```
$ wsp setup group show backend
Group "backend":
  github.com/acme/api-gateway
  github.com/acme/user-service
```

### `wsp setup group delete <name>`

Delete a group. Does not affect the repos themselves.

```
$ wsp setup group delete backend
Deleted group "backend"
```

### Config

### `wsp setup config get <key>`

Get a config value.

### `wsp setup config set <key> <value>`

Set a config value.

### `wsp setup config unset <key>`

Unset a config value.

### `wsp setup config list`

List all config values.

**Available keys:**

| Key              | Description                                                  |
|------------------|--------------------------------------------------------------|
| `branch-prefix`  | Prefix prepended to workspace branch names (`prefix/name`)  |
| `workspaces-dir` | Override the default workspaces directory (`~/dev/workspaces`) |
| `language-integrations.go` | Auto-generate `go.work` when `go.mod` is detected (`true`/`false`) |

### Shell integration

### `wsp setup completion <shell>`

Output shell integration script. Supports `zsh`, `bash`, and `fish`.

```bash
# zsh (~/.zshrc)
eval "$(wsp setup completion zsh)"

# bash (~/.bashrc)
eval "$(wsp setup completion bash)"

# fish (~/.config/fish/config.fish)
wsp setup completion fish | source
```

This provides:

- Tab completion for workspace names, repo shortnames, and group names
- Auto-cd into the workspace directory after `wsp new`
- Auto-cd out of a workspace directory before `wsp rm` if you're inside it
- All other subcommands pass through to the binary unchanged

## Workspaces

### `wsp new <workspace> [repos...] [-g group]`

Create a workspace. Each listed repo gets a local clone checked out to a branch
matching the workspace name. Repos with `@ref` are checked out at that ref as
context repos (no workspace branch created).

| Flag          | Description                |
|---------------|----------------------------|
| `-g, --group` | Include repos from a group |

```
$ wsp new add-billing -g backend web-app proto@v1.0
Creating workspace "add-billing" with 4 repos...
Workspace created: /Users/you/dev/workspaces/add-billing
```

### `wsp repo add [repos...] [-g group]`

Add repos to the current workspace. Must be run from inside a workspace
directory. Supports `@ref` syntax for context repos.

| Flag          | Description                |
|---------------|----------------------------|
| `-g, --group` | Include repos from a group |

```
$ cd ~/dev/workspaces/add-billing
$ wsp repo add proto@v1.0
Adding 1 repos to workspace...
Done.
```

### `wsp repo rm <repos...> [-f]`

Remove repos from the current workspace.

### `wsp repo fetch [--all] [--prune]`

Fetch updates for repos. Runs in parallel.

| Flag      | Description              |
|-----------|--------------------------|
| `--all`   | Fetch all registered repos |
| `--prune` | Prune stale remote branches |

### `wsp ls`

List all workspaces.

```
$ wsp ls
  add-billing  branch:add-billing  repos:3  /Users/you/dev/workspaces/add-billing
  fix-auth     branch:fix-auth     repos:2  /Users/you/dev/workspaces/fix-auth
```

### `wsp st [workspace]`

Show git branch and working tree status for every repo in a workspace. If no
workspace name is given, detects the current workspace from the working
directory.

```
$ wsp st add-billing
Workspace: add-billing  Branch: add-billing

[api-gateway  ]  (add-billing)  3 ahead  2 files changed
[user-service ]  (main       )  clean
[proto        ]  (v1.0       )  clean
```

### `wsp diff [workspace] [-- args]`

Show `git diff` across all repos in a workspace. Extra arguments after `--` are
passed through to `git diff`.

### `wsp rm [workspace] [-f]`

Remove a workspace and its clones. Blocks if any repo has uncommitted work or
unmerged branches. Detects squash-merged branches automatically.

| Flag        | Description                      |
|-------------|----------------------------------|
| `-f, --force` | Force remove even with unmerged branches |

```
$ wsp rm add-billing
Removing workspace "add-billing"...
Workspace "add-billing" removed.
```

### `wsp exec <workspace> -- <command...>`

Run a command in every repo directory of a workspace.

```
$ wsp exec add-billing -- make test
==> [api-gateway] make test
ok

==> [user-service] make test
ok
```

### `wsp cd <workspace>`

Change directory into a workspace. Requires shell integration.

## Context repos (`@ref`)

Some repos are just for reference -- you won't change them. Pin them to a
branch or tag with `@ref`:

```
$ wsp new add-billing api-gateway user-service@main proto@v1.0
```

- `api-gateway` -- checked out on the `add-billing` branch (active)
- `user-service@main` -- checked out at `main` (context, no workspace branch)
- `proto@v1.0` -- checked out at tag `v1.0` (context, detached HEAD)

## Branch prefix

Set a global prefix so every workspace branch is created under your namespace:

```
$ wsp setup config set branch-prefix myname

$ wsp new fix-billing api-gateway
Creating workspace "fix-billing" (branch: myname/fix-billing) with 1 repos...

$ cd ~/dev/workspaces/fix-billing/api-gateway
$ git branch
* myname/fix-billing
```

The workspace directory name stays `fix-billing` -- only the git branch gets
the prefix.

## Shortname resolution

Repos are identified by their full identity (`host/owner/repo`). When names are
unambiguous, `wsp` lets you use shorter names.

| Registered repos                                          | Input         | Resolves to                    |
|-----------------------------------------------------------|---------------|--------------------------------|
| `github.com/acme/api-gateway`, `github.com/acme/web-app` | `api-gateway` | `github.com/acme/api-gateway`  |
| `github.com/acme/utils`, `github.com/other/utils`        | `utils`       | error: ambiguous               |
| `github.com/acme/utils`, `github.com/other/utils`        | `acme/utils`  | `github.com/acme/utils`        |

Resolution walks identity segments right to left and picks the shortest suffix
that uniquely matches one registered repo. If ambiguous, provide more segments.

## Workspace detection

`wsp repo add` and `wsp st` (without arguments) detect the current workspace by
walking up from the working directory until they find a `.wsp.yaml` file:

```
$ cd ~/dev/workspaces/add-billing/api-gateway/src
$ wsp st
Workspace: add-billing  Branch: add-billing
...
```

## Data layout

### Data directory

All `wsp` data is stored under `~/.local/share/wsp/`. Respects `XDG_DATA_HOME`.

```
~/.local/share/wsp/
  config.yaml           registered repos, groups, settings
  mirrors/              bare git clones
```

### Workspaces directory

Workspaces are created under `~/dev/workspaces/` by default. Override with
`wsp setup config set workspaces-dir /path/to/dir`.

### `.wsp.yaml` format

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
branch_prefix: myname

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
