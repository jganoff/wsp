# wsp - Multi-Repo Workspace Manager

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

- Config: `~/.local/share/wsp/config.yaml`
- Mirrors: `~/.local/share/wsp/mirrors/<host>/<user>/<repo>.git/`
- Workspaces: `~/dev/workspaces/<name>/` with `.wsp.yaml` metadata

## CLI Command Structure

Top-level commands use short aliases: `wsp new`, `wsp rm`, `wsp ls`, `wsp st`, `wsp diff`, `wsp exec`, `wsp cd`.

Workspace-scoped repo ops: `wsp repo add`, `wsp repo rm`, `wsp repo fetch`.

All admin/setup commands live under `wsp setup`: `wsp setup repo add/list/remove`, `wsp setup group new/list/show/update/delete`, `wsp setup config list/get/set/unset`, `wsp setup completion zsh|bash|fish`.

When writing docs or examples, use the actual command names above — not the long forms (`remove`, `list`, `status`).

## Security Notes

- **Shell completion scripts** (`src/cli/completion.rs`): User-configurable values (paths, config) embedded in generated shell code must be escaped for the target shell. Single quotes in POSIX shells have no escape mechanism — use `'` → `'\''`. In fish, use `'` → `\'`. Always test with shell metacharacters (`'`, `$`, `` ` ``, newlines) in paths.
- **Path traversal**: `giturl::validate_component()` guards identity parsing. Any new code that builds filesystem paths from user input must go through similar validation.
- `#![deny(unsafe_code)]` is enforced at the crate root.

## Conventions

- Git ops via `std::process::Command`, not libgit2
- Table-driven tests
- YAML config with `serde_yaml_ng`
- Error handling with `anyhow`
- When capturing git output that includes tty-dependent formatting (colors, pagers), pass `--color=always` gated on `std::io::stdout().is_terminal() && !is_json` — see `src/cli/diff.rs` for the pattern
- `build.rs` embeds `git describe` into `WS_VERSION_STRING` for dev/release differentiation

## Releasing

- `just changelog` — preview unreleased changelog
- `just release minor` — dry-run a minor release (also: `patch`, `major`)
- `just release-execute minor` — execute the release

`cargo-release` bumps `Cargo.toml`, runs `git cliff` to regenerate `CHANGELOG.md`, commits, tags `v<version>`, and pushes. The tag push triggers `.github/workflows/release.yml` (cargo-dist) which builds cross-platform binaries, creates a GitHub Release, and publishes to the Homebrew tap (`jganoff/homebrew-tap`).

**Important:** Dry runs (`just release minor`) execute the pre-release hook which modifies `CHANGELOG.md`. Run `git checkout CHANGELOG.md` before the real `--execute` run if the tree is dirty.

Config: `dist-workspace.toml`. After changing dist config (e.g. adding installers), you **must** run `dist generate` (or `dist init` interactively) to regenerate `.github/workflows/release.yml`. The workflow won't include new publish jobs (like `publish-homebrew`) until regenerated.

**cargo-dist config gotcha:** In `dist-workspace.toml`, all fields are flat under `[dist]` — there is no `[dist.homebrew]` subsection. The `tap`, `formula`, and `publish-jobs` keys all go directly under `[dist]`. The Homebrew publish job also requires a `HOMEBREW_TAP_TOKEN` secret in the repo (a PAT with write access to the tap repo).
