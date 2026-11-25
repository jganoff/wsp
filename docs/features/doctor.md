# Feature: `wsp doctor`

Diagnostic command that checks workspace and global state for invariant violations and optionally auto-fixes them.

## Motivation

wsp manages a layered system: global config, mirrors, workspace metadata, and git clones. State can drift when:

- Repos are adopted (origin URL, tracking config may differ from wsp-cloned repos)
- Users manually modify clones (remove remotes, rename branches)
- Operations are interrupted (partial clone, orphaned mirrors)
- Mirrors become orphaned after workspaces are removed

Today, drift is only detected when an operation fails (e.g., `wsp rm` can't determine branch safety). There is no proactive way to check health or fix problems.

`wsp doctor` follows the established pattern from `brew doctor`, `flutter doctor`, and `npm doctor` — a single command that checks everything and reports what's wrong.

## CLI

```
wsp doctor              # check global + workspace state (if inside one)
wsp doctor --fix        # auto-fix what can be fixed
wsp doctor --json       # structured output for agents and CI
```

Top-level command, not under `setup`. This is an operational diagnostic, not admin configuration.

## Check Categories

Checks are organized by scope and priority. Start with P0, add others incrementally.

### Global checks

| Check | Fixable? | Priority | Description |
|-------|----------|----------|-------------|
| Config parseable | No | P0 | `config.yaml` loads without error |
| Mirror exists for each registered repo | Yes (re-clone) | P1 | Every repo in config has a corresponding mirror |
| Orphaned mirrors | Yes (remove) | P2 | Mirrors not referenced by any workspace or config entry |
| Mirror disk usage | No (report) | P2 | Total size of mirrors directory |

### Per-workspace checks

| Check | Fixable? | Priority | Description |
|-------|----------|----------|-------------|
| Repo directory exists | No | P0 | Every repo in `.wsp.yaml` has a directory on disk |
| Origin remote exists | No | P1 | Each repo clone has an `origin` remote |
| Identity matches | Yes (record origin identity in `.wsp.yaml`) | P1 | Origin URL resolves to the identity in `.wsp.yaml` |
| Origin URL matches registered | Yes (repoint) | P0 | Origin URL matches the URL in `config.yaml` |
| Default branch tracks origin | Yes (set upstream) | P2 | Default branch has correct upstream tracking |
| Orphaned directories | No (report) | P1 | Directories in workspace root not in `.wsp.yaml` |

## Output

### Human-readable (default)

```
$ wsp doctor
Checking global state...
  ✓ config is valid
  ✓ 5 registered repos, 5 mirrors

Checking workspace my-feature...
  ✓ api-gateway: ok
  ⚠ bar: origin URL differs from registered URL
      clone:      git@github.com:acme/bar.git
      registered: https://github.com/acme/bar
  ✓ utils: ok
  ✓ all repo directories present

1 warning. Run `wsp doctor --fix` to auto-fix.
```

### `--fix` mode

```
$ wsp doctor --fix
Checking global state...
  ✓ config is valid
  ✓ 5 registered repos, 5 mirrors

Checking workspace my-feature...
  ✓ api-gateway: ok
  ✓ bar: repointed origin URL to https://github.com/acme/bar
  ✓ utils: ok

1 fix applied.
```

### `--json` mode

```json
{
  "ok": false,
  "checks": [
    {
      "scope": "global/config",
      "status": "ok",
      "check": "config-parseable",
      "message": "config is valid"
    },
    {
      "scope": "workspace/my-feature/bar",
      "status": "warn",
      "check": "origin-url-match",
      "message": "origin URL differs from registered URL",
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

## Design decisions

- **Read-only by default.** `--fix` is opt-in. Design tenet: explicit side effects.
- **No hooks/config auditing.** Clones are the developer's space. Doctor only checks invariants wsp depends on.
- **Batch fix, not interactive.** `--fix` fixes everything fixable in one pass. No per-fix prompting. Show what was fixed in the output.
- **Scope detection.** If run inside a workspace, check that workspace. If run outside, check only global state. No flag needed.
- **Exit codes.** 0 = all ok, 1 = warnings found (fixable), 2 = errors found (not fixable).
- **Hardcoded checks.** No plugin system. Add checks as methods. Revisit if check count exceeds ~15.

## Implementation

### Files to create/modify

- `src/cli/doctor.rs` — Command definition and `run` function
- `src/cli/mod.rs` — Register `doctor` command
- `src/output.rs` — `DoctorOutput`, `DoctorCheck` structs
- `src/workspace.rs` — Extract reusable validation helpers (some already exist from adoption)

### Architecture

```rust
struct DoctorCheck {
    scope: String,       // "global/config", "workspace/my-feature/bar"
    check: String,       // "config-parseable", "origin-url-match"
    status: CheckStatus, // Ok, Warn, Error
    message: String,
    fixable: bool,
    details: Option<serde_json::Value>,
}

enum CheckStatus { Ok, Warn, Error }
```

Each check is a function that returns `Vec<DoctorCheck>`. The `run` function collects all checks, optionally applies fixes, and renders output.

### Phases

- [ ] Phase 1: Command skeleton with P0 checks (config parseable, origin URL match, repo dirs exist)
- [ ] Phase 2: `--fix` for origin URL repoint
- [ ] Phase 3: `--json` output
- [ ] Phase 4: P1 checks (mirror exists, origin remote exists, identity matches, orphaned dirs)
- [ ] Phase 5: P2 checks (orphaned mirrors, default branch tracking, disk usage)

## Relationship to other features

- **Repo adoption** — `wsp doctor` can detect and fix drift introduced by adoption (origin URL mismatch, missing tracking). Adoption-time prompts handle the immediate case; doctor handles drift discovered later.
- **Transaction journal** (roadmap) — Doctor reads stale journals to report interrupted operations.
- **Soft-delete** (roadmap) — Doctor reports trash disk usage.
