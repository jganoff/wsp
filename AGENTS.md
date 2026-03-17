# wsp - Multi-Repo Workspace Manager

**Always check [`docs/design-tenets.md`](docs/design-tenets.md) before proposing or implementing changes.** Validate that your approach aligns with the tenets — especially "don't duplicate unix," "just workspace management," and "structured output is the contract." If a proposed feature conflicts with a tenet, flag it.

## Build & Test

Use `just` (see `Justfile`). Key recipes:

- `just` - Default: runs `check` (fmt --check + clippy)
- `just build` - Build release binary (runs check first, regenerates SKILL.md)
- `just test` - Run all tests
- `just ci` - Full CI pipeline (check + build + test + SKILL.md freshness check)
- `just skill` - Regenerate `skills/wsp-manage/SKILL.md` from CLI introspection
- `just fix` - Auto-fix formatting and lint
- `just install-hooks` - Install git pre-commit hook

The `codegen` Cargo feature gates `wsp generate` (hidden command), which introspects clap and serializes sample outputs to produce SKILL.md. `just check` runs clippy with and without this feature. Adding a new command, flag, or output struct automatically updates SKILL.md on next `just build`.

## Architecture

This is a Cargo workspace with two crates:

**`crates/wsp-core/`** — pure library crate (no clap, no owo-colors, no ctrlc)
- `src/config.rs` - Config loading/saving, XDG paths
- `src/git.rs` - Git command execution wrapper
- `src/giturl.rs` - URL parsing and shortname resolution
- `src/mirror.rs` - Bare clone management
- `src/workspace.rs` - Workspace CRUD and clone ops
- `src/gc.rs` - Deferred deletion and recovery (gc pattern)
- `src/output.rs` - All serializable output structs (consumed by both library users and the binary)
- `src/template.rs`, `src/discovery.rs` - Template management
- `src/lang/` - Language integration hooks (Go workspace, etc.)
- `src/agentmd.rs` - AGENTS.md/CLAUDE.md generation

**`crates/wsp/`** — thin binary crate
- `src/main.rs` - Entry point with signal handling
- `src/cli/` - Clap command definitions and dispatch
- `src/output.rs` - Table formatting, ANSI rendering, and `print_gc_warning`

## Context Repos (Removed)

Context repos (pinned to a specific ref via `@ref` syntax) have been removed. All repos in a workspace are active and get the workspace branch. The `@ref` syntax is silently stripped by `parse_repo_ref`. The `WorkspaceRepoRef` struct and `BTreeMap<String, Option<WorkspaceRepoRef>>` type are kept for backward-compatible deserialization of old `.wsp.yaml` files — the `ref` field is ignored at runtime.

## Data Storage

- Config: `~/.local/share/wsp/config.yaml`
- Mirrors: `~/.local/share/wsp/mirrors/<host>/<user>/<repo>.git/`
- Workspaces: `~/dev/workspaces/<name>/` with `.wsp.yaml` metadata
- GC (deferred deletions): `~/.local/share/wsp/gc/<name>__<timestamp>/` with `.wsp-gc.yaml` inside

## CLI Command Structure

Top-level commands use short aliases: `wsp new`, `wsp rm`, `wsp ls`, `wsp st`, `wsp diff`, `wsp exec`, `wsp cd`, `wsp sync`, `wsp log`, `wsp recover`, `wsp rename`.

Workspace-scoped repo ops: `wsp repo add`, `wsp repo rm`, `wsp repo ls`, `wsp repo fetch`.

Admin commands are top-level nouns: `wsp registry add/ls/rm`, `wsp template new/import/ls/show/rm/rename/export/repo/config/agent-md`, `wsp config ls/get/set/unset`, `wsp completion zsh|bash|fish`.

When writing docs or examples, use the actual command names above — not the long forms (`remove`, `list`, `status`).

## Removal Safety & Branch Detection

`wsp rm` and `wsp repo rm` run safety checks before removal. Both `workspace::remove` and `workspace::remove_repos` follow the same pattern:

1. **Pending changes** — `changed_file_count` (dirty working tree) and `ahead_count` (unpushed commits) are checked first. If either is non-zero, removal is blocked.
1b. **Wrong-branch detection** — If HEAD is not on the workspace branch, the workspace branch is checked for unpushed commits separately. This catches the case where a user checked out `main` but has work on the workspace branch.
2. **Fetch with prune** — fetches the mirror from upstream, then propagates to the clone via path-based local fetch with prune. Updates remote tracking refs and clears stale ones (e.g., branches deleted after a PR merge on GitHub). Also removes the legacy `wsp-mirror` remote if present.
3. **Branch safety** — `git::branch_safety()` in `crates/wsp-core/src/git.rs` evaluates the workspace branch against the default branch (`origin/main`). Returns one of four variants, checked in order:

| `BranchSafety` | Meaning | `wsp rm` behavior |
|---|---|---|
| `Merged` | Branch is ancestor of target (regular merge) | Safe, silent removal |
| `SquashMerged` | Tree matches what a squash-merge would produce, or file contents match (`is_content_merged`) | Safe, silent removal |
| `PushedToRemote` | `origin/<branch>` exists but branch is not merged | **Blocked** — requires `--force` |
| `Unmerged` | Branch only exists locally, never pushed | **Blocked** — requires `--force` |

`PushedToRemote` blocks removal to match `git branch -d` semantics: unmerged means unmerged, regardless of whether it's pushed. `--force` is the escape hatch.

### Expected workflow

1. `wsp new my-feature` — creates workspace with branch
2. Make changes, commit, push, open PR (using git directly)
3. PR gets merged (regular, squash, or rebase merge)
4. `wsp rm` — fetches mirror from upstream, propagates to clone (with prune), detects merge via the three-layer check (`branch_is_merged` → `branch_is_squash_merged` → `is_content_merged`), removes workspace

No manual `git fetch` or `git pull` needed — `wsp rm` fetches implicitly via the mirror. If the fetch fails (network issues), the safety check falls back to local data and warns on stderr.

### Edge case: squash merge with conflict resolution

If a squash merge resolved conflicts by changing file contents, `is_content_merged` may return `false` because the branch's files don't match what's on `origin/main`. The workspace will be detected as `Unmerged` and blocked. Use `--force` to remove.

### Deferred deletion (gc)

`wsp rm` moves workspaces to `~/.local/share/wsp/gc/` by default instead of permanently deleting them. This follows git's reflog+gc pattern — users don't know about it until they need recovery:

- `wsp rm` — silently moves to gc. Same UX as before.
- `wsp rm --permanent` — true `fs::remove_dir_all`, bypasses gc.
- `wsp recover` — lists recoverable workspaces, `wsp recover <name>` restores one.
- `gc::maybe_run()` runs after every command (at most once per hour), purging entries older than `gc.retention-days` (default 7, config key `gc.retention-days`).

The gc dir lives alongside mirrors in the XDG data directory (`~/.local/share/wsp/gc/`). `gc::move_dir` uses `fs::rename` when possible, falling back to recursive copy + delete for cross-filesystem moves (EXDEV). GC metadata (`.wsp-gc.yaml`) is written inside the workspace dir before the move.

`workspace::remove(paths, name, force, permanent)` — the `permanent` parameter controls gc vs. immediate deletion. All existing tests pass `permanent: true` to avoid depending on gc internals.

## File Locking

`crates/wsp-core/src/filelock.rs` provides advisory `flock`-based locking via `fs2` to prevent concurrent `wsp` processes from losing writes. Key conventions:

- **Use `with_config()` / `with_metadata()` / `with_template()`** for all read-modify-write operations on `config.yaml`, `.wsp.yaml`, and template YAML files. Never call `load` → modify → `save` directly outside of tests. When adding a new mutation command, always use the appropriate `filelock::with_*` helper.
- **Keep locks short**: Do not hold a lock during network I/O (git clone, git fetch). Use the 3-phase pattern: snapshot under lock → slow I/O without lock → update under lock with re-check.
- **Lock files are not deleted**: The `Drop` impl intentionally leaves `.lock` files on disk to avoid a race where concurrent acquirers end up on different inodes. This is standard `flock` practice.
- Read-only operations (`run_list`, `run_get`, `run_show`) do not need locking.

## Security Notes

- **Shell completion scripts** (`crates/wsp/src/cli/completion.rs`): User-configurable values (paths, config) embedded in generated shell code must be escaped for the target shell. Single quotes in POSIX shells have no escape mechanism — use `'` → `'\''`. In fish, use `'` → `\'`. Always test with shell metacharacters (`'`, `$`, `` ` ``, newlines) in paths. Clap's `COMPLETE=zsh` output calls `compdef` which requires `compinit` — the zsh codepath guards against this with a no-op stub + stderr warning. Any changes to the zsh completion path must preserve this guard.
- **Shell startup resilience**: `wsp completion` is `eval`'d at shell startup. It must **never** fail — config load errors, version skew, or corrupt files must degrade gracefully (warn on stderr, emit completions with hooks disabled). Any future command that gets sourced into a shell must follow this pattern.
- **Path traversal**: `giturl::validate_component()` guards identity parsing. Any new code that builds filesystem paths from user input must go through similar validation.
- `#![deny(unsafe_code)]` is enforced at the crate root.
- **Platform-specific code**: wsp ships on macOS, Linux, and Windows. Never use `std::os::unix` or `std::os::windows` without `#[cfg(unix)]` / `#[cfg(windows)]` guards. See `crates/wsp-core/src/agentmd.rs` for the pattern. `just check-cross` type-checks against Windows and Linux targets to catch this locally.

## Naming

The project was renamed from `ws` to `wsp`. User-facing identifiers all use `wsp`:
- CLI binary: `wsp`
- Metadata file: `.wsp.yaml`
- Git remote: clones only have `origin` (no wsp-specific remotes)
- Env var: `WSP_SHELL`
- Shell vars: `wsp_bin`, `wsp_root`, `wsp_dir`
- Data dir: `~/.local/share/wsp/`
- Brew formula: `wsp`

Internal Rust variable names (`ws_dir`, `ws_bin` parameters) are kept as shorthand for "workspace" and are NOT product identifiers — don't rename them.

**CLAUDE.md is a symlink to AGENTS.md** — wsp generates AGENTS.md in workspace roots (with `<!-- wsp:begin/end -->` markers) and creates CLAUDE.md as a symlink to it. Edits to either file modify the same underlying file. Do not replace the symlink with a regular file. The `wsp doctor` W9 check validates this relationship.

## Conventions

- Git ops via `std::process::Command`, not libgit2
- Table-driven tests
- YAML config with `serde_yaml_ng`
- Error handling with `anyhow`
- When capturing git output that includes tty-dependent formatting (colors, pagers), pass `--color=always` gated on `std::io::stdout().is_terminal() && !is_json` — see `crates/wsp/src/cli/diff.rs` for the pattern
- `build.rs` embeds `git describe` into `WSP_VERSION_STRING` for dev/release differentiation
- Clap `visible_alias`/`alias` dispatches under the primary command name — only match the primary name in dispatch arms (e.g., `Some(("ls", m))` not `Some(("ls", m)) | Some(("list", m))`)
- Commands that don't modify workspace/config/repo state get `[read-only]` in their `.about()` text. This propagates to `--help` and SKILL.md automatically via clap introspection. Add it when creating new read-only commands.
- **Shell completions are mandatory**: Every flag and positional arg that accepts a known set of values (workspace names, template names, repo identities) must have an `ArgValueCandidates` completer. Completers live in `crates/wsp/src/cli/completers.rs`. File-path arguments (e.g., `-f`/`--file`) use `value_hint(FilePath)` for shell-native path completion.
- **Roadmap hygiene**: When a feature ships, remove its section from `docs/roadmap.md` entirely — don't mark checkboxes as done. Commit roadmap removals in the same commit as the feature code, not separately.

## Gotchas

- **Adding fields to `Config`**: The `Config` struct uses `#[derive(Default)]` for production code. Search for `Config {` across the codebase when adding new fields. Also update `crates/wsp/src/cli/cfg.rs` (`run_list`, `run_get`, `run_set`, `run_unset`) to handle the new config key, add it to `complete_config_keys()` in `crates/wsp/src/cli/completers.rs` (with value completions in `complete_config_values()` if applicable), and document it in the `config` help topic in `crates/wsp/src/cli/help.rs`.
- **Adding fields to `Metadata`**: Test helpers in `crates/wsp-core/src/lang/go.rs`, `crates/wsp-core/src/lang/mod.rs`, and `crates/wsp-core/src/workspace.rs` tests have manual `Metadata { ... }` initializers. Search for `Metadata {` across the codebase when adding new fields. Also check if `workspace::create()` / `create_inner()` need a corresponding parameter — their signatures mirror Metadata fields, and ~20 test call sites use `create()` directly.
- **Adding fields to `WorkspaceRepoRef`**: Test helpers in `crates/wsp-core/src/workspace.rs`, `crates/wsp-core/src/agentmd.rs`, `crates/wsp-core/src/filelock.rs`, and `crates/wsp-core/src/lang/go.rs` have manual `WorkspaceRepoRef { ... }` initializers. Search for `WorkspaceRepoRef {` across the codebase when adding new fields.
- **Adding fields to `Template`**: The `Template` struct in `crates/wsp-core/src/template.rs` has manual initializers in `crates/wsp-core/src/discovery.rs` tests and `crates/wsp-core/src/filelock.rs` tests. Search for `Template {` across the codebase when adding new fields. All new fields should be `Option` with `#[serde(default, skip_serializing_if = "Option::is_none")]` for backward compatibility.
- **Adding fields to `Paths`**: `Paths` has manual initializers in `crates/wsp-core/src/config.rs` (`resolve`, `from_dirs`), `crates/wsp/src/cli/status.rs` (`dummy_paths`), and `crates/wsp-core/src/gc.rs` (`test_paths`). Search for `Paths {` across the codebase when adding new fields.
- **Adding fields to output structs**: `RepoStatusEntry`, `RepoDiffEntry`, `RepoLogEntry`, `ExecRepoResult`, `SyncRepoResult`, `SyncAbortRepoResult`, and `MutationOutput` have constructors spread across CLI files (`status.rs`, `diff.rs`, `log.rs`, `exec.rs`, `sync.rs`, `new.rs`, `rename.rs`, `delete.rs`, etc.), codegen `sample()` methods in `crates/wsp-core/src/output.rs`, and 5-15 test sites in `crates/wsp/src/output.rs`. Search for `StructName {` across the codebase when adding new fields. After changes, run `just skill` to regenerate SKILL.md. **Note:** The `Output` enum does not derive `Debug`, so tests cannot use `.unwrap_err()` on `Result<Output>` — use `.is_err()` + `.err().unwrap()` instead.
- **Custom `help` subcommand**: Clap registers a built-in `help` subcommand by default. The app uses `.disable_help_subcommand(true)` and provides its own `help` command in `crates/wsp/src/cli/help.rs` that supports topic pages (e.g., `wsp help wspignore`). The `help` dispatch happens early in `crates/wsp/src/main.rs` before `Paths::resolve()` so it works even if config is broken.
- **Adding commands or output structs**: The `codegen` feature gates SKILL.md generation. When adding a new command, it appears in SKILL.md automatically via clap introspection. When adding a new output struct, add a `#[cfg(feature = "codegen")] sample()` method in `crates/wsp-core/src/output.rs` and wire it in `crates/wsp/src/cli/skill.rs`. Run `just skill` to regenerate. `just ci` will fail if SKILL.md is stale. Every new command should have both `.about()` (short, shown by `-h`) and `.long_about()` (conceptual description of what it does and why, shown by `--help`). Keep `long_about` focused on mental model and behavior — don't repeat flag details.
- **Adding skills**: New skills in `skills/` need a corresponding `include_str!` constant in `crates/wsp-core/src/agentmd.rs` and must be wired into `install_skill()` to be installed into workspaces. Also add the skill to the `managed` and `managed_dirs` sets in `check_claude_dir()` in `crates/wsp-core/src/workspace.rs` — otherwise `wsp rm`/`wsp rename` will flag the skill directory as user content and block removal. Run `/check-skill-registration` to verify.
- **`git.*` config keys are validated against a denylist**: `wsp config set git.*` and `wsp template config set git.*` reject keys that can execute arbitrary commands (e.g. `core.sshCommand`, `core.hooksPath`, `filter.*`). The denylist lives in `crates/wsp-core/src/config.rs::DANGEROUS_GIT_CONFIG_KEY_PREFIXES` and the validator is `config::validate_git_config_key()`. Any new code path that writes git config from user input **must** call this validator before writing. Template import (`load_from_file`) also validates on load so malicious shared templates can't bypass the interactive check.
- **`cargo install --path .` is broken**: The root `Cargo.toml` is a virtual workspace manifest (no `[package]`). `cargo install --path .` will fail with "found a virtual manifest". Always use `cargo install --path crates/wsp` for local installs. `cargo install --git ...` is unaffected — Cargo auto-discovers the binary from workspace members.
- **Test remote URLs**: `giturl::parse()` only handles SSH (`git@host:path`) and HTTPS URLs — not local filesystem paths. Tests that need identity validation from a remote URL must use `git@test.local:user/repo.git` style URLs, not the temp-dir paths used by `setup_test_env()` for upstream URLs.
- **Interactive prompts**: `read_stdin_line()` in `crates/wsp-core/src/util.rs` returns `"\n"` when the user presses Enter but `""` (empty, no newline) on EOF or pipe close. Ctrl-C is handled globally (`process::exit(130)` in the ctrlc handler), but interactive commands should still detect EOF by checking for empty string before trimming — see `crates/wsp/src/cli/setup.rs:read_prompt()` as the pattern. Every interactive command should gate on `std::io::stdin().is_terminal()` and provide a non-interactive fallback.
- **Default dispatch uses root-level ArgMatches**: `list::run` and `status::run` are called from the default dispatch path (`cli/mod.rs`, no subcommand) with root-level `ArgMatches` that lack subcommand-specific args. Use `try_get_one().ok().flatten()` (not `get_flag()`) to safely handle missing args without panicking.
- **CLI changes require regeneration**: After adding/changing commands, flags, or output structs, run `just skill` to regenerate SKILL.md and manpages. `just ci` checks freshness and will fail if stale.
- **Updating user-facing docs**: Always cross-reference `skills/wsp-manage/SKILL.md` (auto-generated from clap) when updating README.md or docs/usage.md. SKILL.md is the ground truth for what commands, flags, and subcommands the binary actually exposes.
- **Config key naming**: Config keys use dot-separated groups: `git.<key>`, `lang.<name>`, `shell.tmux`, `shell.prompt`. Old names (`git_config.*`, `language-integrations.*`, `experimental.*`) are accepted via `normalize_key()` in `crates/wsp-core/src/template.rs` and produce a deprecation warning on stderr. The YAML fields use `rename` + `alias` serde attributes for backward-compatible deserialization. When adding new config keys, use dot-separated groups.
- **Experimental features**: Features labeled experimental are listed in `config::EXPERIMENTAL_KEYS` (display-only, no gating mechanism). They are always functional — the label just signals "may change." To mark a new feature as experimental, add its key to `EXPERIMENTAL_KEYS`. When it graduates, remove it from the list. The old `experimental` config section with its `enabled` gate is kept for backward-compat deserialization only (`skip_serializing`) and migrated to top-level fields on load.
- **Adding features that touch invariants**: When adding a new feature that introduces state (config fields, metadata fields, managed files, remotes), consider whether `wsp doctor` should validate it. Doctor checks live in `crates/wsp/src/cli/doctor.rs` as standalone helper functions (`check_*`). Each check emits a `DoctorCheck` with scope, status, fixable flag, and optional details JSON. Fixable checks must handle both `--fix` and report-only modes. New check types automatically appear in `--json` output. **Design rule: every Warn/Error check must either be auto-fixable (`fixable: true`) or include actionable guidance in the message telling the user exactly what command to run.** Warnings without a path to resolution are noise — downgrade to Ok (informational) or don't emit them.
- **Workspace root content checks**: `check_root_content()` returns `Vec<RootProblem>` (not strings). Helper functions (`check_agents_md`, `check_claude_md`, `check_claude_dir`, `check_go_work`) also return `RootProblem`/`Vec<RootProblem>`. The hardcoded skip list in `check_root_content` includes `.wsp.yaml`, `.wsp.yaml.lock`, `.wspignore`, and repo dirs. OS noise files (`.DS_Store` etc.) are handled by wspignore patterns, not hardcoded. When adding new wsp-managed root files, add them to the skip list in `check_root_content`.

## Releasing

- `just changelog` — preview unreleased changelog
- `just release minor` — dry-run a minor release (also: `patch`, `major`)
- `just release-execute minor` — execute the release

`cargo-release` bumps `Cargo.toml`, runs `git cliff` to regenerate `CHANGELOG.md`, commits, tags `v<version>`, and pushes. The tag push triggers `.github/workflows/release.yml` (cargo-dist) which builds cross-platform binaries, creates a GitHub Release, and publishes to the Homebrew tap (`jganoff/homebrew-tap`).

**Important:** Dry runs (`just release minor`) execute the pre-release hook which modifies `CHANGELOG.md`. Run `git checkout CHANGELOG.md` before the real `--execute` run if the tree is dirty.

Config: `dist-workspace.toml`. After changing dist config (e.g. adding installers), you **must** run `dist generate` (or `dist init` interactively) to regenerate `.github/workflows/release.yml`. The workflow won't include new publish jobs (like `publish-homebrew`) until regenerated.

**cargo-dist config gotcha:** In `dist-workspace.toml`, all fields are flat under `[dist]` — there is no `[dist.homebrew]` subsection. The `tap`, `formula`, and `publish-jobs` keys all go directly under `[dist]`. The Homebrew publish job also requires a `HOMEBREW_TAP_TOKEN` secret in the repo (a PAT with write access to the tap repo).
