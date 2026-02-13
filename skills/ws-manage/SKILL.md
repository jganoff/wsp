---
name: ws-manage
description: Manage multi-repo workspaces with ws
user_invocable: true
---

# ws — Multi-Repo Workspace Manager

Use `ws` to manage workspaces that span multiple git repositories. Each workspace creates local clones from bare mirror clones, sharing a single branch name across repos.

**Always use `--json` when calling ws programmatically.** JSON output goes to stdout; progress messages go to stderr.

## Quick Reference

### Repos (global registry)

```bash
ws setup repo add <git-url>        # Register + bare-clone a repo
ws setup repo list --json          # List registered repos
ws setup repo remove <name>        # Remove repo + mirror
```

### Groups (named sets of repos)

```bash
ws setup group new <name> <repo>...      # Create a group
ws setup group list --json               # List groups
ws setup group show <name> --json        # Show repos in a group
ws setup group delete <name>             # Delete a group
ws setup group update <name> --add <repo>... --remove <repo>...
```

### Workspaces

```bash
ws new <name> <repo>... [--group <g>]   # Create workspace with local clones
ws ls --json                             # List all workspaces
ws st [<name>] --json                   # Git status across repos
ws diff [<name>] [-- <git-diff-args>] --json  # Git diff across repos
ws repo add <repo>... [--group <g>]     # Add repos to current workspace
ws repo rm <repo>... [-f]               # Remove repos from current workspace
ws repo fetch [--all] [--prune]         # Fetch updates (parallel)
ws rm [<name>] [-f]                     # Remove workspace + clones
ws exec <name> -- <command>             # Run command in each repo
ws cd <name>                            # cd into workspace (shell integration)
```

### Config

```bash
ws setup config get branch-prefix --json
ws setup config set branch-prefix <value>
ws setup config unset branch-prefix
```

### Skill management

```bash
ws setup skill install                 # Install this skill to ~/.claude/skills/
```

## JSON Output Schemas

### `ws setup repo list --json`
```json
{"repos": [{"identity": "github.com/org/repo", "shortname": "repo", "url": "git@github.com:org/repo.git"}]}
```

### `ws ls --json`
```json
{"workspaces": [{"name": "my-ws", "branch": "my-ws", "repo_count": 3, "path": "/home/user/dev/workspaces/my-ws"}]}
```

### `ws st --json`
```json
{"workspace": "my-ws", "branch": "my-ws", "repos": [{"name": "repo-a", "branch": "my-ws", "ahead": 0, "changed": 2, "status": "2 modified"}]}
```

### `ws diff --json`
```json
{"repos": [{"name": "repo-a", "diff": "--- a/file\n+++ b/file\n..."}]}
```

### `ws setup config get <key> --json`
```json
{"key": "branch-prefix", "value": "myname"}
```

### Mutation commands (add, remove, new, etc.)
```json
{"ok": true, "message": "Registered github.com/org/repo"}
```

### Errors
```json
{"error": "repo \"foo\" not found"}
```

## Shortname Resolution

Repos are identified by `host/owner/repo` (e.g., `github.com/acme/api-gateway`). You can use the shortest unique suffix:
- `api-gateway` if unambiguous
- `acme/api-gateway` to disambiguate from `other-org/api-gateway`

## `@ref` Syntax for Context Repos

When creating a workspace, pin a repo to a specific branch/tag/SHA:
```bash
ws new my-feature api-gateway user-service@main proto@v1.0
```
- `api-gateway` — active repo, gets the workspace branch
- `user-service@main` — context repo, checked out at `main`
- `proto@v1.0` — context repo, checked out at tag `v1.0`

## Directory Layout

```
~/dev/workspaces/<workspace-name>/
  .ws.yaml              # Workspace metadata
  <repo-name>/          # Local clone for each repo
```

## Common Agent Workflows

### Create a workspace and start working
```bash
ws setup repo list --json                     # See available repos
ws new my-feature api-gateway user-service    # Create workspace
cd ~/dev/workspaces/my-feature                # Enter workspace
```

### Check what's changed
```bash
ws st --json          # From inside a workspace
ws diff --json        # See all diffs
```

### Run tests across all repos
```bash
ws exec my-feature -- make test
```

### Clean up when done
```bash
ws rm my-feature      # Removes clones + branch (if merged)
ws rm my-feature -f   # Force remove even if unmerged
```
