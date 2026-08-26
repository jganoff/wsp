# Architecture

## Overview

wsp is a Cargo workspace split into two crates:

**`crates/wsp-core/`** — pure library crate. No clap, no owo-colors, no ctrlc. Usable as a library by tools that embed wsp logic.

| Module | Purpose |
|--------|---------|
| `src/config.rs` | Config loading/saving, XDG paths, dangerous git key denylist |
| `src/git.rs` | Git command execution wrapper |
| `src/giturl.rs` | URL parsing and shortname resolution |
| `src/mirror.rs` | Bare clone management |
| `src/workspace.rs` | Workspace CRUD and clone ops |
| `src/gc.rs` | Deferred deletion and recovery (gc pattern) |
| `src/output.rs` | All serializable output structs (used by both library users and the binary) |
| `src/template.rs` | Template management |
| `src/discovery.rs` | Template discovery |
| `src/lang/` | Language integration hooks (Go workspace, etc.) |
| `src/agentmd.rs` | AGENTS.md/CLAUDE.md generation and skill installation |
| `src/filelock.rs` | Advisory flock-based locking via fs2 |

**`crates/wsp/`** — thin binary crate. CLI definitions, table rendering, ANSI output.

| Module | Purpose |
|--------|---------|
| `src/main.rs` | Entry point with signal handling |
| `src/cli/` | Clap command definitions and dispatch |
| `src/output.rs` | Table formatting, ANSI rendering, `print_gc_warning` |

## Dependency Rule

**`crates/xtask/`** — repo automation, never shipped. Tasks run against the
repository rather than a workspace: drafting release notes today, more later.
Reached through `just` recipes. Kept out of `crates/wsp` so the product stays a
workspace manager.

`crates/wsp-core` must not depend on `clap`, `owo-colors`, or `ctrlc`. Keep it embeddable. All CLI concerns belong in `crates/wsp`.

## Context Repos (Removed)

Context repos (pinned to a specific ref via `@ref` syntax) were removed. All repos in a workspace are active and get the workspace branch. The `@ref` syntax is silently stripped by `parse_repo_ref`. `WorkspaceRepoRef` and `BTreeMap<String, Option<WorkspaceRepoRef>>` are kept for backward-compatible deserialization of old `.wsp.yaml` files — the `ref` field is ignored at runtime.
