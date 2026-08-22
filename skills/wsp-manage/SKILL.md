---
name: wsp-manage
description: Manage multi-repo workspaces with wsp
user_invocable: true
---

# wsp — Multi-Repo Workspace Manager

Use `wsp` to manage workspaces that span multiple git repositories. Each workspace creates local clones from bare mirror clones, sharing a single branch name across repos.

**Always use `--json` when calling wsp programmatically.** JSON output goes to stdout; progress messages go to stderr.

## Quick Reference

### Registry (global repo registry)

```bash
wsp registry add [<url>] [--from <from>] [--pattern <pattern>] [--all] [--ssh] [--https] [--no-discover] # Register and bare-clone a repository
wsp registry ls                                 # List registered repositories [read-only] (alias: list)
wsp registry rm <name>                          # Remove a repository and its mirror (alias: remove)
```

### Templates (shareable workspace definitions)

```bash
wsp template new <name> [<repos>]... [-w <from-workspace>] [-f <file>] [-d <description>] # Create a new template
wsp template import <file> [--name <name>] [--update] [--force] # Import a template from a .wsp.yaml file
wsp template ls                                 # List all templates [read-only] (alias: list)
wsp template show <name>                        # Show template contents [read-only]
wsp template rm <name>                          # Remove a template (alias: remove)
wsp template rename <old> <new> [--force]       # Rename a template
wsp template export <name> [--stdout]           # Export a template to a file or stdout [read-only]
wsp template repo                               # Add or remove repos in a template
wsp template config                             # Manage template config overrides
wsp template agent-md                           # Manage template AGENTS.md content
wsp template setup-commands                     # Manage per-repo setup commands in a template
```

### Workspaces

```bash
wsp new [<workspace>] [-b <branch>] [<repos>]... [-t <template>] [-w <from-workspace>] [-f <file>] [--empty] [--no-fetch] [-d <description>] [--no-discover] # Create a new workspace (alias: create)
wsp ls [-t] [-U] [-r]                           # List active workspaces [read-only] (alias: list)
wsp st [<workspace>] [-v]                       # Git status across workspace repos [read-only] (alias: status)
wsp diff [<workspace>] [<args>]...              # Show git diff across workspace repos [read-only]
wsp log [<workspace>] [--oneline] [<args>]...   # Show commits ahead of upstream per workspace repo [read-only]
wsp sync [<workspace>] [--strategy <strategy>] [--dry-run] [--abort] [--no-discover] # Fetch and rebase/merge all workspace repos
wsp exec [<workspace>] <command>...             # Run a command in each repo of a workspace
wsp cd <workspace>                              # Change directory into a workspace
wsp rm [<workspace>] [-f] [-y]                  # Remove a workspace (alias: remove)
wsp recover [<workspace>]                       # List, inspect, or restore recently removed workspaces [read-only without args]
wsp rename <old> [<new>]                        # Rename a workspace, its directory, and git branches
wsp repo add [<repos>]... [-t <template>] [--no-discover] [--no-fetch] # Add repos to current workspace
wsp repo rm <repos>... [-f]                     # Remove repo(s) from the current workspace (alias: remove)
wsp repo fetch [--all] [--prune]                # Fetch updates for workspace repos
wsp repo ls                                     # List repos in the current workspace [read-only] (alias: list)
wsp repo setup [<repos>]... [--all]             # Run setup commands for repos in the current workspace
wsp repo setup-commands                         # Manage per-repo setup commands
```

### Config

```bash
wsp config ls [--global]                        # List all config values [read-only] (alias: list)
wsp config get <key> [--global]                 # Get a config value [read-only]
wsp config set <key> <value> [--global]         # Set a config value
wsp config unset <key> [--global]               # Unset a config value
```

### Diagnostics

```bash
wsp doctor [--fix]                              # Check workspace and global state for problems
```

## JSON Output Schemas

### `wsp registry ls --json`
```json
{
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "url": "git@github.com:acme/api-gateway.git"
    }
  ]
}
```

### `wsp ls --json`
```json
{
  "workspaces": [
    {
      "name": "my-feature",
      "branch": "my-feature",
      "repo_count": 2,
      "path": "/home/user/dev/workspaces/my-feature",
      "description": "migrating billing to stripe v3",
      "created": "2026-03-01T10:00:00+00:00",
      "last_used": "2026-03-06T15:30:00+00:00",
      "created_from": "backend"
    }
  ]
}
```

### `wsp st --json`
```json
{
  "workspace": "my-feature",
  "branch": "my-feature",
  "workspace_dir": "/home/user/dev/workspaces/my-feature",
  "description": "migrating billing to stripe v3",
  "created": "2026-01-15T10:00:00Z",
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "path": "/home/user/dev/workspaces/my-feature/api-gateway",
      "branch": "my-feature",
      "ahead": 2,
      "behind": 0,
      "changed": 1,
      "has_upstream": true,
      "role": "active"
    }
  ]
}
```

### `wsp diff --json`
```json
{
  "workspace": "my-feature",
  "branch": "my-feature",
  "workspace_dir": "/home/user/dev/workspaces/my-feature",
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "path": "/home/user/dev/workspaces/my-feature/api-gateway",
      "diff": "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n+use std::io;\n ..."
    }
  ]
}
```

### `wsp log --json`
```json
{
  "workspace": "my-feature",
  "branch": "my-feature",
  "workspace_dir": "/home/user/dev/workspaces/my-feature",
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "path": "/home/user/dev/workspaces/my-feature/api-gateway",
      "commits": [
        {
          "hash": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
          "authored_at": "2023-11-14T22:13:20+00:00",
          "subject": "feat: add billing endpoint"
        }
      ]
    }
  ]
}
```

### `wsp sync --json`
```json
{
  "workspace": "my-feature",
  "branch": "my-feature",
  "dry_run": false,
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "path": "/home/user/dev/workspaces/my-feature/api-gateway",
      "action": "rebase onto origin/main",
      "ok": true,
      "detail": "2 commit(s) rebased"
    }
  ]
}
```

### `wsp sync --abort --json`
```json
{
  "workspace": "my-feature",
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "path": "/home/user/dev/workspaces/my-feature/api-gateway",
      "action": "skip",
      "ok": true
    },
    {
      "identity": "github.com/acme/user-service",
      "shortname": "user-service",
      "path": "/home/user/dev/workspaces/my-feature/user-service",
      "action": "rebase aborted",
      "ok": true
    }
  ]
}
```

### `wsp repo ls --json`
```json
{
  "workspace": "my-feature",
  "branch": "my-feature",
  "workspace_dir": "/home/user/dev/workspaces/my-feature",
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "dir_name": "api-gateway"
    },
    {
      "identity": "github.com/acme/shared-lib",
      "shortname": "shared-lib",
      "dir_name": "shared-lib"
    }
  ]
}
```

### `wsp exec <workspace> --json -- <command>`
```json
{
  "workspace": "my-feature",
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "path": "/home/user/dev/workspaces/my-feature/api-gateway",
      "directory": "api-gateway",
      "exit_code": 0,
      "ok": true,
      "stdout": "hello\n"
    }
  ]
}
```

### `wsp repo fetch --json`
```json
{
  "workspace": "my-feature",
  "repos": [
    {
      "identity": "github.com/acme/api-gateway",
      "shortname": "api-gateway",
      "ok": true
    }
  ]
}
```

### `wsp template ls --json`
```json
{
  "templates": [
    {
      "name": "backend",
      "repo_count": 3
    }
  ]
}
```

### `wsp template show <name> --json`
```json
{
  "name": "backend",
  "repos": [
    {
      "url": "git@github.com:acme/api-gateway.git",
      "identity": "github.com/acme/api-gateway"
    },
    {
      "url": "git@github.com:acme/user-service.git",
      "identity": "github.com/acme/user-service"
    }
  ]
}
```

### `wsp config ls --json`
```json
{
  "settings": [
    {
      "key": "branch-prefix",
      "value": "jg"
    },
    {
      "key": "workspaces-dir",
      "value": "~/dev/workspaces"
    },
    {
      "key": "sync-strategy",
      "value": "rebase",
      "source": "workspace"
    }
  ]
}
```

### `wsp config get <key> --json`
```json
{
  "key": "branch-prefix",
  "value": "jg"
}
```

### `Mutation commands (new, rm, add, remove, set, etc.)`
```json
{
  "ok": true,
  "message": "Registered github.com/acme/api-gateway"
}
```

### `wsp registry add --from <org> --all --json`
```json
{
  "registered": [
    "github.com/acme/api-gateway",
    "github.com/acme/user-service"
  ],
  "skipped": [
    "github.com/acme/shared-lib"
  ]
}
```

### `wsp recover --json`
```json
{
  "workspaces": [
    {
      "name": "my-feature",
      "branch": "jganoff/my-feature",
      "trashed_at": "2026-01-01T00:00:00Z",
      "original_path": "~/dev/workspaces/my-feature",
      "repo_count": 3
    }
  ],
  "retention_days": 7
}
```

### `wsp recover show <name> --json`
```json
{
  "entry": {
    "name": "my-feature",
    "branch": "jganoff/my-feature",
    "trashed_at": "2026-01-01T00:00:00Z",
    "original_path": "~/dev/workspaces/my-feature",
    "repos": [
      "github.com/acme/api-gateway",
      "github.com/acme/user-service"
    ],
    "disk_bytes": 52428800,
    "gc_path": "~/.local/share/wsp/gc/my-feature__20260101T000000.000"
  },
  "retention_days": 7
}
```

### `wsp doctor --json`
```json
{
  "ok": false,
  "checks": [
    {
      "scope": "global",
      "check": "config-parseable",
      "status": "ok",
      "message": "config is valid (5 registered repos)"
    },
    {
      "scope": "workspace/my-feature/bar",
      "check": "origin-url-match",
      "status": "warn",
      "message": "bar: origin URL differs from registered URL",
      "fixable": true,
      "details": {
        "clone_url": "git@github.com:acme/bar.git",
        "registered_url": "https://github.com/acme/bar"
      }
    }
  ],
  "summary": {
    "total": 8,
    "ok": 7,
    "warn": 1,
    "error": 0,
    "fixed": 0
  }
}
```

### `Errors`
```json
{
  "error": "repo \"foo\" not found"
}
```

## Shortname Resolution

Repos are identified by `host/owner/repo` (e.g., `github.com/acme/api-gateway`). You can use the shortest unique suffix:
- `api-gateway` if unambiguous
- `acme/api-gateway` to disambiguate from `other-org/api-gateway`

## Directory Layout

```
~/dev/workspaces/<workspace-name>/
  .wsp.yaml              # Workspace metadata
  <repo-name>/          # Local clone for each repo
```

## Common Agent Workflows

### Create a workspace and start working
```bash
wsp registry ls --json                         # See available repos
wsp new my-feature api-gateway user-service    # Create workspace
cd ~/dev/workspaces/my-feature                # Enter workspace
```

### Check what's changed
```bash
wsp st --json          # From inside a workspace
wsp diff --json        # See all diffs
```

### Sync with upstream
```bash
wsp sync --json        # Fetch + rebase all repos
wsp sync --strategy merge --json  # Use merge instead of rebase
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
