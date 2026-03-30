# wsp - Multi-Repo Workspace Manager

**Always check [`docs/design-tenets.md`](docs/design-tenets.md) before proposing or implementing changes.** Validate that your approach aligns with the tenets — especially "don't duplicate unix," "just workspace management," and "structured output is the contract."

## Quick Reference

| What | Where |
|------|-------|
| All CLI commands | [`skills/wsp-manage/SKILL.md`](skills/wsp-manage/SKILL.md) (auto-generated) |
| Architecture & module map | [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) |
| Removal safety algorithm | [`docs/features/removal-safety.md`](docs/features/removal-safety.md) |
| Release workflow | `/wsp-release` skill |

## After Cloning

```bash
just setup    # install pre-commit hook (run once; prevents fmt/lint failures in CI)
```

## Build & Test

```bash
just          # check (fmt + clippy)
just build    # release binary (runs check, regenerates SKILL.md)
just test     # all tests
just ci       # full pipeline (check + build + test + SKILL.md freshness)
just fix      # auto-fix fmt and lint
```

Run `just fix` before `just ci` when you've made changes — `just ci` starts with `cargo fmt --check` and fails immediately on unformatted code. Running `just ci` before pushing is an optimization; CI will catch failures on the PR automatically, but it saves a round-trip.

The `codegen` feature gates `wsp generate`, which introspects clap to produce SKILL.md. `just check` runs clippy with and without it. Adding a command or output struct updates SKILL.md automatically on next `just build`.

## Data Storage

- Config: `~/.local/share/wsp/config.yaml`
- Mirrors: `~/.local/share/wsp/mirrors/<host>/<user>/<repo>.git/`
- Workspaces: `~/dev/workspaces/<name>/` with `.wsp.yaml` metadata
- GC (deferred deletions): `~/.local/share/wsp/gc/<name>__<timestamp>/` with `.wsp-gc.yaml`

## File Locking

Use `filelock::with_config()` / `with_metadata()` / `with_template()` for all read-modify-write operations. Never call `load` → modify → `save` directly outside of tests. Keep locks short: do not hold during network I/O. Use the 3-phase pattern: snapshot under lock → slow I/O → update under lock with re-check.

## Security

- **Shell completions** (`completion.rs`): escape user values for target shell. POSIX: `'` → `'\''`. Fish: `'` → `\'`. Completion must never fail at shell startup — degrade gracefully.
- **Path traversal**: new code building paths from user input must use `giturl::validate_component()`.
- **Git config keys**: validate with `config::validate_git_config_key()` before writing. One canonical denylist in `config.rs` — do not add a second one.
- `#![deny(unsafe_code)]` enforced at both crate roots.
- **Platform code**: guard with `#[cfg(unix)]` / `#[cfg(windows)]`. See `agentmd.rs` for the pattern.

## Naming

Product: binary `wsp`, metadata `.wsp.yaml`, env `WSP_SHELL`, shell vars `wsp_bin`/`wsp_root`/`wsp_dir`, data dir `~/.local/share/wsp/`. Internal Rust names (`ws_dir`, `ws_bin`) are shorthand, not product identifiers.

**CLAUDE.md is a symlink to AGENTS.md** — do not replace the symlink with a regular file.

**Before proposing a new command name**, check the open GitHub issues (labels P1--P4) — planned command names are reserved there. A name collision means either the existing issue needs to be closed/updated first, or a different name is needed.

## Conventions

- Git ops via `std::process::Command`, not libgit2
- Table-driven tests; property-based tests where applicable
- YAML config with `serde_yaml_ng`; error handling with `anyhow`
- Git output with tty formatting: pass `--color=always` gated on `stdout().is_terminal() && !is_json`
- Read-only commands get `[read-only]` in `.about()`. Every flag accepting known values needs an `ArgValueCandidates` completer.
- Clap dispatch: only match primary command name (e.g., `Some(("ls", m))` — not aliases).
- **Boolean "mode" flags + positional args**: `ArgGroup` only enforces mutual exclusion among named flags — it does not cover positional args. If a mode flag (e.g. `--empty`) is incompatible with positionals (e.g. repo args), add an explicit early bail in `run()` or use `.conflicts_with("repos")` on the flag's `Arg` definition.
- **`--force` vs `--yes` — two distinct flags, two distinct jobs**:
  - `--force` / `-f`: overrides a **failed safety check** (unmerged branches, dirty worktree, protection rules). Changes what the tool does. Implies `--yes` — a user who explicitly overrides a guard has already signaled intent. Never needed when all checks pass.
  - `--yes` / `-y`: skips an **interactive confirmation prompt** when all preconditions are met. Behavior is identical either way; the tool just doesn't ask. Required for non-TTY callers that can't respond to prompts.
  - Litmus test: does the flag change the operation, or just whether the tool asks first? Changes behavior = `--force`. Skips a prompt = `--yes`. `--yes` alone never overrides a failed safety check.
  - Real-world precedent: `gh repo delete --yes` (no safety check, just confirm), `git push --force` (overrides fast-forward guard), `terraform apply -auto-approve` (skips prompt, no guard override).
- **Destructive confirmation prompts**: any command that destroys data must prompt `[y/N]` when stdin is a TTY. Use `--yes` / `-y` (short) to skip the prompt — this is the project-wide convention (matches `gh`, `npm`, `apt`). Non-TTY callers without `--yes` must get a clear error telling them to pass `--yes`. Never silently skip or silently proceed. Pattern: check `std::io::stdin().is_terminal()`, bail with `"pass --yes to confirm: wsp <cmd> --yes"` if not interactive and `--yes` not set.
- When a feature ships, close the corresponding GitHub issue and remove its section from `docs/roadmap.md` entirely (don't check boxes).

## Gotchas

- **Changing `workspace::remove` / `remove_repos` signatures**: callers exist in `workspace.rs` tests, `gc.rs` tests, and `crates/wsp/src/cli/`. Search all three — `gc.rs` is easy to miss.
- **Changing `workspace::create` or `workspace::add_repos` signatures**: both have multiple test call sites in `workspace.rs` (~30 for `create`, ~6 for `add_repos`). Use perl to update tests: `perl -0777 -pi -e 's/(None|Some\("[^"]*"\)),(\s+)&upstream_urls,/$1,$2None,$2\&upstream_urls,/g'` as a model for inserting new `Option` args before `upstream_urls`. Also note: `clone_from_mirror` is called from both `create_inner` and `add_repos` — update both call sites.
- **Adding fields to `Config`, `Metadata`, `WorkspaceRepoRef`, `Template`, `Paths`, or output structs**: search `StructName {` across the codebase and update all manual initializers. For output structs also run `just skill`. For `Config` also update `cfg.rs`, completers, and `help.rs`. `Template` derives `Default` — new optional fields need no test updates; write new test literals as `Template { field: value, ..Default::default() }` rather than filling every field.
- **`git.*` config keys**: one canonical denylist in `config.rs::DANGEROUS_GIT_CONFIG_KEY_PREFIXES`. `workspace::is_dangerous_git_config_key()` and `template::apply_config()` both delegate to it — do not add a separate list.
- **`cargo install --path .` is broken** — virtual workspace root has no `[package]`. Use `cargo install --path crates/wsp`.
- **wsp-core visibility**: Use `pub(crate)` for anything not needed by `crates/wsp`. Internal helpers (file I/O, stdin, collision detection) should not leak into the public library API.
- **Test remote URLs**: use `git@test.local:user/repo.git` style, not temp-dir paths.
- **Tests that call `git commit`**: always configure `user.email`, `user.name`, and `commit.gpgsign=false` on the repo first — CI runners have no global git identity. Use `testutil::local_commit` (which handles this) rather than calling git directly.
- **macOS path canonicalization in tests**: `tempfile::tempdir()` returns `/var/folders/...` but `git` returns `/private/var/folders/...` (macOS `/var` → `/private/var` symlink). Any test comparing a git-returned path against a temp-dir path must call `.canonicalize()` on both sides, or the assertion will silently fail on macOS.
- **Adding skills**: wire into `agentmd.rs::install_skill()`, register in `workspace.rs::check_claude_dir()` managed + managed_dirs sets. Run `/check-skill-registration` to verify.
- **Adding commands, flags, or output structs**: run `just skill` after. `just ci` fails if SKILL.md is stale. Flag changes to existing commands also trigger staleness.
- **CLI changes**: every new command needs `.about()` (short) and `.long_about()` (conceptual). Shell completers are mandatory for known-value args.
- **Adding features that touch invariants**: consider whether `wsp doctor` should validate it. Every Warn/Error check must be auto-fixable or include actionable guidance.
- **Adding a contextual hint**: add a branch in `crates/wsp/src/hints.rs::evaluate()`, gated on `hint_enabled(cfg, "<key>")`. Document the suppression key (`advice.<key>`) in the `config` topic in `help.rs`. Hints must be state-driven — never random or time-based. Suppress via `wsp config set advice.<key> false`; suppress all via `wsp config set hints false`.
- **`help` subcommand**: custom implementation in `cli/help.rs`. Dispatches before `Paths::resolve()` so it works even with broken config.
- **Default dispatch** uses root-level `ArgMatches` — use `try_get_one().ok().flatten()` not `get_flag()`.
- **Interactive prompts**: gate on `stdin().is_terminal()`. EOF returns `""`, Enter returns `"\n"` — detect EOF before trimming.
- **Config key naming**: dot-separated groups (`git.<key>`, `lang.<name>`, `shell.tmux`). Old names normalized via `normalize_key()` with deprecation warning.
- **Workspace root skip list**: `check_root_content()` hardcodes `.wsp.yaml`, `.wsp.yaml.lock`, `.wspignore`, repo dirs. Add new wsp-managed root files here.

## Releasing

See `/wsp-release` skill for the multi-step process. Key gotcha: dry-run modifies `CHANGELOG.md` — run `git checkout CHANGELOG.md` before executing.

**Do not manually edit `release.yml`.** It is fully owned by `cargo-dist` — any hand-edits (e.g. SHA-pinning actions) will be overwritten the next time `dist generate` runs, and the `dist plan` freshness check in CI will fail on every PR until they are reverted. If you need SHA-pinned actions, use Dependabot (`dependabot.yml`, ecosystem `github-actions`) instead.
