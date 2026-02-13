# ws - Multi-Repo Workspace Manager

## Build & Test

Use `just` (see `Justfile`). Key recipes:

- `just` - Default: runs `check` (fmt --check + clippy)
- `just build` - Build release binary (runs check first)
- `just test` - Run all tests
- `just ci` - Full CI pipeline (check + build + test)
- `just fix` - Auto-fix formatting and lint
- `just install-hooks` - Install git pre-commit hook

## Architecture

- `src/main.rs` - Entry point with signal handling
- `src/cli/` - Clap command definitions
- `src/config.rs` - Config loading/saving, XDG paths
- `src/git.rs` - Git command execution wrapper
- `src/giturl.rs` - URL parsing and shortname resolution
- `src/mirror.rs` - Bare clone management
- `src/workspace.rs` - Workspace CRUD and clone ops
- `src/group.rs` - Group management
- `src/output.rs` - Table formatting and status display

## Data Storage

- Config: `~/.local/share/ws/config.yaml`
- Mirrors: `~/.local/share/ws/mirrors/<host>/<user>/<repo>.git/`
- Workspaces: `~/dev/workspaces/<name>/` with `.ws.yaml` metadata

## CLI Command Structure

Top-level commands use short aliases: `ws new`, `ws rm`, `ws ls`, `ws st`, `ws diff`, `ws exec`, `ws cd`.

Workspace-scoped repo ops: `ws repo add`, `ws repo rm`, `ws repo fetch`.

All admin/setup commands live under `ws setup`: `ws setup repo add/list/remove`, `ws setup group new/list/show/update/delete`, `ws setup config list/get/set/unset`, `ws setup completion zsh|bash|fish`.

When writing docs or examples, use the actual command names above — not the long forms (`remove`, `list`, `status`).

## Conventions

- Git ops via `std::process::Command`, not libgit2
- Table-driven tests
- YAML config with `serde_yml`
- Error handling with `anyhow`
- When capturing git output that includes tty-dependent formatting (colors, pagers), pass `--color=always` gated on `std::io::stdout().is_terminal() && !is_json` — see `src/cli/diff.rs` for the pattern
- `build.rs` embeds `git describe` into `WS_VERSION_STRING` for dev/release differentiation

## Releasing

- `just changelog` — preview unreleased changelog
- `just release minor` — dry-run a minor release (also: `patch`, `major`)
- `just release-execute minor` — execute the release

`cargo-release` bumps `Cargo.toml`, runs `git cliff` to regenerate `CHANGELOG.md`, commits, tags `v<version>`, and pushes. The tag push triggers `.github/workflows/release.yml` (cargo-dist) which builds cross-platform binaries, creates a GitHub Release, and publishes to the Homebrew tap (`jganoff/homebrew-tap`).

**Important:** Dry runs (`just release minor`) execute the pre-release hook which modifies `CHANGELOG.md`. Run `git checkout CHANGELOG.md` before the real `--execute` run if the tree is dirty.

Config: `dist-workspace.toml`. To regenerate CI workflows after changing dist config, run `dist init` interactively — `dist generate` does not work reliably without interactive mode for installer changes.
