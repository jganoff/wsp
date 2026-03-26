# Usage

Full command reference and configuration guide for `wsp`.

## Registry

### `wsp registry add <url>`

Register a repository and create its bare mirror.

```
$ wsp registry add git@github.com:acme/api-gateway.git
Cloning git@github.com:acme/api-gateway.git...
Registered github.com/acme/api-gateway
```

### `wsp registry ls`

List all registered repositories.

```
$ wsp registry ls
  github.com/acme/api-gateway [api-gateway]  (git@github.com:acme/api-gateway.git)
  github.com/acme/user-service [user-service]  (git@github.com:acme/user-service.git)
```

Shows identity, shortname (in brackets), and URL.

### `wsp registry rm <name>`

Remove a repository and delete its bare mirror. Accepts a shortname.

```
$ wsp registry rm api-gateway
Removing mirror for github.com/acme/api-gateway...
Removed github.com/acme/api-gateway
```

## Templates

Templates are sharable workspace definitions — a named set of repos and
optional settings. They replace the older "groups" feature.

### `wsp template new <name> [repos...]`

Create a named template from repos or derive from an existing workspace.

```
$ wsp template new backend api-gateway user-service
Created template "backend" with 2 repos
```

Derive from a workspace:

```
$ wsp template new backend -w add-billing
Created template "backend" from workspace "add-billing"
```

### `wsp template ls`

List all templates.

```
$ wsp template ls
  backend (2 repos)
  frontend (1 repos)
```

### `wsp template show <name>`

Show the repos and config in a template.

```
$ wsp template show backend
Template "backend":
  github.com/acme/api-gateway
  github.com/acme/user-service
```

### `wsp template repo add <name> <repos...>`

Add repos to a template. Idempotent — repos already present are skipped with a
warning.

```
$ wsp template repo add backend git@github.com:acme/proto.git
Template "backend":
  github.com/acme/api-gateway
  github.com/acme/user-service
  github.com/acme/proto
```

### `wsp template repo rm <name> <repos...>`

Remove repos from a template. Accepts URLs, identities, or shortnames.

```
$ wsp template repo rm backend proto
Template "backend":
  github.com/acme/api-gateway
  github.com/acme/user-service
```

### `wsp template config set <name> <key> <value>`

Set a template config override. Template config overrides global config when
creating workspaces from the template.

```
$ wsp template config set backend sync-strategy merge
template "backend": sync-strategy = merge
```

**Valid template config keys:**

| Key pattern              | Value          |
|--------------------------|----------------|
| `sync-strategy`          | `rebase` or `merge` |
| `language-integrations.<name>` | `true` or `false` |
| `git_config.<key>`       | string         |

Hyphens and underscores are interchangeable in key names (e.g., `git_config.` and
`git-config.` are equivalent).

### `wsp template config get <name> <key>`

Get a template config value. Returns `(not set)` for absent keys.

### `wsp template config unset <name> <key>`

Remove a template config override.

### `wsp template agent-md set <name> <path>`

Set AGENTS.md content for a template from a file. Use `-` for stdin.

```
$ wsp template agent-md set backend ./project-rules.md
template "backend": agent-md set

$ echo "# Rules" | wsp template agent-md set backend -
template "backend": agent-md set
```

### `wsp template agent-md unset <name>`

Clear AGENTS.md content from a template.

### `wsp template rm <name>`

Remove a template. Does not affect workspaces created from it.

### `wsp template export <name>`

Export a template as a shareable `.wsp.yaml` file.

```
$ wsp template export backend
Wrote backend.wsp.yaml
```

## Config

### `wsp config get <key>`

Get a config value.

### `wsp config set <key> <value>`

Set a config value.

### `wsp config unset <key>`

Unset a config value.

### `wsp config ls`

List all config values.

**Available keys:**

| Key              | Description                                                  |
|------------------|--------------------------------------------------------------|
| `branch-prefix`  | Prefix prepended to workspace branch names (`prefix/name`)  |
| `workspaces-dir` | Override the default workspaces directory (`~/dev/workspaces`) |
| `language-integrations.go` | Auto-generate `go.work` when `go.mod` is detected (`true`/`false`) |
| `agent-md`       | Auto-generate `AGENTS.md` in workspaces (`true`/`false`, default `true`) |
| `gc.retention-days` | Days to keep removed workspaces before permanent deletion (default `7`) |

## Shell integration

### `wsp completion <shell>`

Output shell integration script. Supports `zsh`, `bash`, and `fish`.

```bash
# zsh (~/.zshrc)
eval "$(wsp completion zsh)"

# bash (~/.bashrc)
eval "$(wsp completion bash)"

# fish (~/.config/fish/config.fish)
wsp completion fish | source
```

This provides:

- Tab completion for workspace names, repo shortnames, and template names
- Auto-cd into the workspace directory after `wsp new`
- Auto-cd out of a workspace directory before `wsp rm` if you're inside it
- All other subcommands pass through to the binary unchanged

## Workspaces

### `wsp new <workspace> [repos...] [-t template]`

Create a workspace. Each listed repo gets a local clone checked out to a branch
matching the workspace name.

| Flag             | Description                   |
|------------------|-------------------------------|
| `-t, --template` | Include repos from a template |
| `-w, --workspace` | Derive repos from an existing workspace |
| `-f, --file`     | Create from a `.wsp.yaml` file |

```
$ wsp new add-billing -t backend web-app proto
Creating workspace "add-billing" with 4 repos...
Workspace created: /Users/you/dev/workspaces/add-billing
```

### `wsp repo add [repos...] [-t template]`

Add repos to the current workspace. Must be run from inside a workspace
directory.

| Flag             | Description                   |
|------------------|-------------------------------|
| `-t, --template` | Include repos from a template |

```
$ cd ~/dev/workspaces/add-billing
$ wsp repo add proto
Adding 1 repos to workspace...
Done.
```

### `wsp repo rm <repos...> [-f]`

Remove repos from the current workspace.

### `wsp repo ls`

List repos in the current workspace.

### `wsp repo setup [repos...] [--force]`

Run setup commands declared in a repo's `.wsp.yaml`. Prompts for approval if
not already approved; skips silently in non-interactive environments.

| Flag        | Description                                      |
|-------------|--------------------------------------------------|
| `--force`   | Re-prompt even if commands are already approved  |

```
$ wsp repo setup
Setup commands for github.com/acme/api-gateway:
  task setup
  lefthook install
Run these commands? [y/N] y
  $ task setup
  $ lefthook install
Setup complete: 1 ran, 0 skipped.
```

Repos with no `setup_commands` in their `.wsp.yaml` are silently skipped.
When no repos are given, runs for all repos in the current workspace that
have `setup_commands`.

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
[user-service ]  (add-billing)  clean
```

### `wsp diff [workspace] [-- args]`

Show `git diff` across all repos in a workspace. Extra arguments after `--` are
passed through to `git diff`.

### `wsp log [workspace] [-- args]`

Show `git log` across all repos in a workspace. Extra arguments after `--` are
passed through to `git log`.

### `wsp sync [workspace] [--strategy merge]`

Fetch and rebase (default) or merge all repos in a workspace.

| Flag                | Description                         |
|---------------------|-------------------------------------|
| `--strategy merge`  | Use merge instead of rebase         |
| `--abort`           | Abort an in-progress rebase/merge   |

### `wsp rm [workspace] [-f]`

Remove a workspace. Blocks if any repo has uncommitted work or unmerged
branches. Detects squash-merged branches automatically.

Removed workspaces are recoverable via `wsp recover` (kept for 7 days by
default).

| Flag            | Description                              |
|-----------------|------------------------------------------|
| `-f, --force`   | Force remove even with unmerged branches |

```
$ wsp rm add-billing
Removing workspace "add-billing"...
Workspace "add-billing" removed.
```

### `wsp recover [workspace]`

List recoverable workspaces, or restore one by name.

```
$ wsp recover
  add-billing  removed 2h ago  3 repos
  old-feature  removed 3d ago  2 repos

$ wsp recover add-billing
Recovered workspace "add-billing"
```

### `wsp rename <old> <new>`

Rename a workspace.

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

## Branch prefix

Set a global prefix so every workspace branch is created under your namespace:

```
$ wsp config set branch-prefix myname

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
  config.yaml           registered repos, templates, settings
  approvals.yaml        approved setup_commands decisions (keyed by hash)
  templates/            saved workspace templates
  mirrors/              bare git clones
  gc/                   deferred deletions (recoverable)
```

### Workspaces directory

Workspaces are created under `~/dev/workspaces/` by default. Override with
`wsp config set workspaces-dir /path/to/dir`.

### `.wsp.yaml` format

```yaml
name: add-billing
branch: add-billing
repos:
  github.com/acme/api-gateway:
    url: git@github.com:acme/api-gateway.git
  github.com/acme/user-service:
    url: git@github.com:acme/user-service.git
config:
  language_integrations:
    go: true
created: 2025-06-15T11:00:00Z
```

The `url` field captures the URL used at creation time, making the file
shareable as a template. Any `.wsp.yaml` can be used to create a new workspace
via `wsp new -f path/to/.wsp.yaml`.

#### Per-repo `.wsp.yaml` — `setup_commands`

Individual repos can declare post-clone setup commands in their own `.wsp.yaml`
(checked into the repo root, not the workspace root):

```yaml
setup_commands:
  - task setup
  - lefthook install
```

`wsp` runs these after cloning the repo into a workspace. Because
`setup_commands` allows arbitrary shell execution, `wsp` shows the commands
and prompts for approval before running (like `direnv allow`):

```
Setup commands for github.com/acme/api-gateway:
  task setup
  lefthook install
Run these commands? [y/N]
```

- **`y`** — allow and run; future clones skip the prompt automatically
- **`N` / Enter** — skip

Approvals are stored by a SHA-256 hash of the command list in
`~/.local/share/wsp/approvals.yaml`. If the command list changes (e.g. a new
step is added upstream), `wsp` re-prompts automatically.

In non-interactive environments (CI, pipes), `wsp` skips setup commands and
prints a notice. Run `wsp repo setup` manually to approve and execute them.

`wsp doctor` (check W15) warns when a repo has unapproved setup commands.
`wsp doctor --fix` invokes the interactive approval flow.

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
```
