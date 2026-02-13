# Feature Roadmap

Prioritized feature plan for wsp, organized by shipping priority.

## P0 — Daily Workflow Loop

### `wsp sync`

**Complexity:** Medium

Fetch + rebase/merge all repos in one command, workspace-aware.

- Active repos: fetch origin, rebase (default) or merge workspace branch onto upstream default branch
- Context repos: fetch and update to pinned ref
- `--strategy rebase|merge` (default: rebase, configurable via `wsp setup config set sync-strategy`)
- `--dry-run` to preview
- Parallel fetch, serial rebase. Stop cleanly on conflicts.

```
$ wsp sync
api-gateway   fetch + rebase onto origin/main   ok (2 commits rebased)
user-service  fetch + fast-forward origin/main   ok (already up to date)
proto         fetch + checkout v1.0               ok (ref unchanged)
```

- [ ] Implement fetch all (parallel)
- [ ] Implement rebase/merge per active repo (serial)
- [ ] Handle context repo ref updates
- [ ] Conflict detection and clean stop
- [ ] `--strategy` flag and config default
- [ ] `--dry-run`
- [ ] `--json` output

### `wsp push`

**Complexity:** Small

Push all active repos that have commits ahead of upstream. Skip context repos.

```
$ wsp push
api-gateway   push add-billing -> origin   ok
user-service  (context @main)              skipped
proto         (context @v1.0)              skipped
```

- [ ] Detect repos with commits ahead
- [ ] Skip context repos
- [ ] `--set-upstream` for first push
- [ ] `--force-with-lease` for post-rebase
- [ ] `--json` output

## P1 — High Value, Low Effort

### `wsp log`

**Complexity:** Small

Unified cross-repo commit log showing commits ahead of upstream per active repo.

```
$ wsp log
api-gateway
  a1b2c3d  feat: add billing endpoint     (2 hours ago)
  d4e5f6a  refactor: extract handler      (3 hours ago)

user-service
  (no commits on workspace branch)

$ wsp log --oneline
api-gateway  a1b2c3d  feat: add billing endpoint      2h ago
api-gateway  d4e5f6a  refactor: extract handler        3h ago
```

- [ ] `git log @{upstream}..HEAD` per active repo
- [ ] `--oneline` flat chronological view across repos
- [ ] Pass-through git log args via `--`
- [ ] `--json` output

### `wsp run` (enhanced exec)

**Complexity:** Medium

Smarter `wsp exec` with filtering and parallelism.

| Flag | Behavior |
|------|----------|
| `--changed` | Only repos with uncommitted changes or commits ahead |
| `--active` | Only active repos (skip context) |
| `--parallel` | Run in parallel |
| `--bail` | Stop on first failure |
| `--repo <name>` | Specific repos only |

```
$ wsp run --changed -- make test
Running in 1 of 3 repos (--changed)...
==> [api-gateway] make test
ok
```

- [ ] `--changed` filter (reuse ahead_count / changed_file_count)
- [ ] `--active` filter
- [ ] `--parallel` with interleaved output
- [ ] `--bail` semantics
- [ ] `--repo` filter
- [ ] Make `exec` an alias for `run` with no filters
- [ ] `--json` output

## P2 — Team Adoption

### `wsp export` / `wsp new --from`

**Complexity:** Small

Shareable workspace templates for reproducible workspace creation.

```
$ wsp export add-billing --file
Wrote add-billing.wsp-template.yaml

$ wsp new --from add-billing.wsp-template.yaml
```

Template format:

```yaml
name: add-billing
repos:
  - api-gateway
  - user-service@main
  - proto@v1.0
```

- [ ] `wsp export <name>` (prints `wsp new` one-liner)
- [ ] `wsp export <name> --file` (writes `.wsp-template.yaml`)
- [ ] `wsp new --from <file>` reads template
- [ ] Keep templates explicit (repo lists, not group references)

### `wsp pr`

**Complexity:** Medium-Large

Open PRs across all active repos via `gh`, with cross-repo linking.

```
$ wsp pr --link
api-gateway    https://github.com/acme/api-gateway/pull/42      created
user-service   (no commits ahead)                                 skipped

$ wsp pr --title "Add billing" --draft --link
```

With `--link`, each PR body includes:

```
## Related PRs (wsp workspace: add-billing)
- acme/api-gateway#42
- acme/user-service#43
```

- [ ] Detect repos with commits ahead (reuse push logic)
- [ ] Shell out to `gh pr create`
- [ ] `--title`, `--body`, `--draft` flags
- [ ] `--link` cross-referencing (create PRs, then update bodies with links)
- [ ] `--json` output

## P3 — Later

### Workspace Hooks

**Complexity:** Medium

Lifecycle hooks (`on-create`, `on-sync`, `on-push`) in config or per-group.

```yaml
# in config.yaml
hooks:
  on-create:
    - command: make setup
      repos: active

# or per-group
groups:
  backend:
    repos: [api-gateway, user-service]
    hooks:
      on-create:
        - command: make setup
```

- [ ] Hook config schema (global and per-group)
- [ ] `on-create` lifecycle event
- [ ] `on-sync` lifecycle event
- [ ] `on-push` lifecycle event
- [ ] Hook execution via `wsp run` internals

### `wsp import`

**Complexity:** Medium

Auto-discover and register repos from a GitHub/GitLab org.

```
$ wsp import github.com/acme --pattern "api-*,user-*"
Registered 5 repos.

$ wsp import github.com/acme --all
```

- [ ] `gh api` integration to list org repos
- [ ] `--pattern` glob filtering
- [ ] `--all` flag
- [ ] Interactive picker (nice-to-have)
- [ ] GitLab support (later)

## Design Principles

- Every command is **workspace-aware** (active vs. context repos, workspace vs. upstream branches)
- Daily ops are **top-level short commands** (`sync`, `push`, `log`, `run`)
- **Always support `--json`** for scripting and AI agents
- **Parallel by default** for reads, serial for writes
- No new external dependencies unless justified (`gh` for PR ops is the exception)
