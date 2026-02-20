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

Workspace-scoped repo ops: `wsp repo add`, `wsp repo rm`, `wsp repo ls`, `wsp repo fetch`.

All admin/setup commands live under `wsp setup`: `wsp setup repo add/list/remove`, `wsp setup group new/list/show/update/delete`, `wsp setup config list/get/set/unset`, `wsp setup completion zsh|bash|fish`.

When writing docs or examples, use the actual command names above — not the long forms (`remove`, `list`, `status`).

## Removal Safety & Branch Detection

`wsp rm` and `wsp repo rm` run safety checks before deleting. Both `workspace::remove` and `workspace::remove_repos` follow the same pattern:

1. **Pending changes** — `changed_file_count` (dirty working tree) and `ahead_count` (unpushed commits) are checked first. If either is non-zero, removal is blocked.
2. **Fetch with prune** — `git fetch --prune origin` updates remote tracking refs and clears stale ones (e.g., branches deleted after a PR merge on GitHub).
3. **Branch safety** — `git::branch_safety()` in `src/git.rs` evaluates the workspace branch against the default branch (`origin/main`). Returns one of four variants, checked in order:

| `BranchSafety` | Meaning | `wsp rm` behavior |
|---|---|---|
| `Merged` | Branch is ancestor of target (regular merge) | Safe, silent removal |
| `SquashMerged` | Tree matches what a squash-merge would produce, or file contents match (`is_content_merged`) | Safe, silent removal |
| `PushedToRemote` | `origin/<branch>` exists but branch is not merged | **Blocked** — requires `--force` |
| `Unmerged` | Branch only exists locally, never pushed | **Blocked** — requires `--force` |

`PushedToRemote` blocks removal to match `git branch -d` semantics: unmerged means unmerged, regardless of whether it's pushed. `--force` is the escape hatch.

### Expected workflow

1. `wsp new my-feature` — creates workspace with branch
2. Make changes, commit, push, open PR
3. PR gets merged (regular, squash, or rebase merge)
4. `wsp rm` — fetches origin (with prune), detects merge via the three-layer check (`branch_is_merged` → `branch_is_squash_merged` → `is_content_merged`), removes workspace

No manual `git fetch` or `git pull` needed — `wsp rm` fetches implicitly. If the fetch fails (network issues), the safety check falls back to local data and warns on stderr.

### Edge case: squash merge with conflict resolution

If a squash merge resolved conflicts by changing file contents, `is_content_merged` may return `false` because the branch's files don't match what's on `origin/main`. The workspace will be detected as `Unmerged` and blocked. Use `--force` to remove.

## Security Notes

- **Shell completion scripts** (`src/cli/completion.rs`): User-configurable values (paths, config) embedded in generated shell code must be escaped for the target shell. Single quotes in POSIX shells have no escape mechanism — use `'` → `'\''`. In fish, use `'` → `\'`. Always test with shell metacharacters (`'`, `$`, `` ` ``, newlines) in paths.
- **Path traversal**: `giturl::validate_component()` guards identity parsing. Any new code that builds filesystem paths from user input must go through similar validation.
- `#![deny(unsafe_code)]` is enforced at the crate root.

## Naming

The project was renamed from `ws` to `wsp`. User-facing identifiers all use `wsp`:
- CLI binary: `wsp`
- Metadata file: `.wsp.yaml`
- Git remote: `wsp-mirror`
- Env var: `WSP_SHELL`
- Shell vars: `wsp_bin`, `wsp_root`, `wsp_dir`
- Data dir: `~/.local/share/wsp/`
- Brew formula: `wsp`

Internal Rust variable names (`ws_dir`, `ws_bin` parameters) are kept as shorthand for "workspace" and are NOT product identifiers — don't rename them.

## Conventions

- Git ops via `std::process::Command`, not libgit2
- Table-driven tests
- YAML config with `serde_yaml_ng`
- Error handling with `anyhow`
- When capturing git output that includes tty-dependent formatting (colors, pagers), pass `--color=always` gated on `std::io::stdout().is_terminal() && !is_json` — see `src/cli/diff.rs` for the pattern
- `build.rs` embeds `git describe` into `WSP_VERSION_STRING` for dev/release differentiation
- Clap `visible_alias`/`alias` dispatches under the primary command name — only match the primary name in dispatch arms (e.g., `Some(("ls", m))` not `Some(("ls", m)) | Some(("list", m))`)

## Releasing

- `just changelog` — preview unreleased changelog
- `just release minor` — dry-run a minor release (also: `patch`, `major`)
- `just release-execute minor` — execute the release

`cargo-release` bumps `Cargo.toml`, runs `git cliff` to regenerate `CHANGELOG.md`, commits, tags `v<version>`, and pushes. The tag push triggers `.github/workflows/release.yml` (cargo-dist) which builds cross-platform binaries, creates a GitHub Release, and publishes to the Homebrew tap (`jganoff/homebrew-tap`).

**Important:** Dry runs (`just release minor`) execute the pre-release hook which modifies `CHANGELOG.md`. Run `git checkout CHANGELOG.md` before the real `--execute` run if the tree is dirty.

Config: `dist-workspace.toml`. After changing dist config (e.g. adding installers), you **must** run `dist generate` (or `dist init` interactively) to regenerate `.github/workflows/release.yml`. The workflow won't include new publish jobs (like `publish-homebrew`) until regenerated.

**cargo-dist config gotcha:** In `dist-workspace.toml`, all fields are flat under `[dist]` — there is no `[dist.homebrew]` subsection. The `tap`, `formula`, and `publish-jobs` keys all go directly under `[dist]`. The Homebrew publish job also requires a `HOMEBREW_TAP_TOKEN` secret in the repo (a PAT with write access to the tap repo).
