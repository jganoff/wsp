---
name: wsp-manage
description: Manage multi-repo workspaces with wsp
user_invocable: true
---

# wsp — Multi-Repo Workspace Manager

Use `wsp` to manage workspaces that span multiple git repositories. Each workspace creates local clones from bare mirror clones, sharing a single branch name across repos.

**Always use `--json` when calling wsp programmatically.** JSON output goes to stdout; progress messages go to stderr.

## Quick Reference

### Repos (global registry)

```bash
wsp setup repo add <git-url>        # Register + bare-clone a repo
wsp setup repo list --json          # List registered repos
wsp setup repo remove <name>        # Remove repo + mirror
```

### Groups (named sets of repos)

```bash
wsp setup group new <name> <repo>...      # Create a group
wsp setup group list --json               # List groups
wsp setup group show <name> --json        # Show repos in a group
wsp setup group delete <name>             # Delete a group
wsp setup group update <name> --add <repo>... --remove <repo>...
```

### Workspaces

```bash
wsp new <name> <repo>... [--group <g>]   # Create workspace with local clones
wsp ls --json                             # List all workspaces
wsp st [<name>] --json                   # Git status across repos
wsp diff [<name>] [-- <git-diff-args>] --json  # Git diff across repos
wsp repo add <repo>... [--group <g>]     # Add repos to current workspace
wsp repo rm <repo>... [-f]               # Remove repos from current workspace
wsp repo fetch [--all] [--prune]         # Fetch updates (parallel)
wsp rm [<name>] [-f]                     # Remove workspace + clones
wsp exec <name> -- <command>             # Run command in each repo
wsp cd <name>                            # cd into workspace (shell integration)
```

### Config

```bash
wsp setup config get branch-prefix --json
wsp setup config set branch-prefix <value>
wsp setup config unset branch-prefix
```

### Skill management

```bash
wsp setup skill install                 # Install this skill to ~/.claude/skills/
```

## JSON Output Schemas

### `wsp setup repo list --json`
```json
{"repos": [{"identity": "github.com/org/repo", "shortname": "repo", "url": "git@github.com:org/repo.git"}]}
```

### `wsp ls --json`
```json
{"workspaces": [{"name": "my-ws", "branch": "my-ws", "repo_count": 3, "path": "/home/user/dev/workspaces/my-ws"}]}
```

### `wsp st --json`
```json
{"workspace": "my-ws", "branch": "my-ws", "repos": [{"name": "repo-a", "branch": "my-ws", "ahead": 0, "changed": 2, "status": "2 modified"}]}
```

### `wsp diff --json`
```json
{"repos": [{"name": "repo-a", "diff": "--- a/file\n+++ b/file\n..."}]}
```

### `wsp setup config get <key> --json`
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
wsp new my-feature api-gateway user-service@main proto@v1.0
```
- `api-gateway` — active repo, gets the workspace branch
- `user-service@main` — context repo, checked out at `main`
- `proto@v1.0` — context repo, checked out at tag `v1.0`

## Directory Layout

```
~/dev/workspaces/<workspace-name>/
  .wsp.yaml              # Workspace metadata
  <repo-name>/          # Local clone for each repo
```

## Common Agent Workflows

### Create a workspace and start working
```bash
wsp setup repo list --json                     # See available repos
wsp new my-feature api-gateway user-service    # Create workspace
cd ~/dev/workspaces/my-feature                # Enter workspace
```

### Check what's changed
```bash
wsp st --json          # From inside a workspace
wsp diff --json        # See all diffs
```

### Run tests across all repos
```bash
wsp exec my-feature -- make test
```

### Clean up when done
```bash
wsp rm my-feature      # Removes clones + branch (if merged)
wsp rm my-feature -f   # Force remove even if unmerged
```
