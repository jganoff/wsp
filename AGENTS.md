# wsp - Multi-Repo Workspace Manager

**Always check [`docs/design-tenets.md`](docs/design-tenets.md) before proposing or implementing changes.** Validate that your approach aligns with the tenets — especially "don't duplicate unix," "just workspace management," and "structured output is the contract."

## Quick Reference

| What | Where |
|------|-------|
| All CLI commands | [`skills/wsp-manage/SKILL.md`](skills/wsp-manage/SKILL.md) (auto-generated) |
| Architecture & module map | [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) |
| Removal safety algorithm | [`docs/features/removal-safety.md`](docs/features/removal-safety.md) |
| Release workflow | `/wsp-release` skill |

## Build & Test

```bash
just          # check (fmt + clippy)
just build    # release binary (runs check, regenerates SKILL.md)
just test     # all tests
just ci       # full pipeline (check + build + test + SKILL.md freshness)
just fix      # auto-fix fmt and lint
```

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

## Conventions

- Git ops via `std::process::Command`, not libgit2
- Table-driven tests; property-based tests where applicable
- YAML config with `serde_yaml_ng`; error handling with `anyhow`
- Git output with tty formatting: pass `--color=always` gated on `stdout().is_terminal() && !is_json`
- Read-only commands get `[read-only]` in `.about()`. Every flag accepting known values needs an `ArgValueCandidates` completer.
- Clap dispatch: only match primary command name (e.g., `Some(("ls", m))` — not aliases).
- When a feature ships, remove its section from `docs/roadmap.md` entirely (don't check boxes).

## Gotchas

- **Changing `workspace::remove` / `remove_repos` signatures**: callers exist in `workspace.rs` tests, `gc.rs` tests, and `crates/wsp/src/cli/`. Search all three — `gc.rs` is easy to miss.
- **Adding fields to `Config`, `Metadata`, `WorkspaceRepoRef`, `Template`, `Paths`, or output structs**: search `StructName {` across the codebase and update all manual initializers. For output structs also run `just skill`. For `Config` also update `cfg.rs`, completers, and `help.rs`.
- **`git.*` config keys**: one canonical denylist in `config.rs::DANGEROUS_GIT_CONFIG_KEY_PREFIXES`. `workspace::is_dangerous_git_config_key()` and `template::apply_config()` both delegate to it — do not add a separate list.
- **`cargo install --path .` is broken** — virtual workspace root has no `[package]`. Use `cargo install --path crates/wsp`.
- **wsp-core visibility**: Use `pub(crate)` for anything not needed by `crates/wsp`. Internal helpers (file I/O, stdin, collision detection) should not leak into the public library API. `publish = false` is intentional until the surface is clean — see issue #19.
- **Test remote URLs**: use `git@test.local:user/repo.git` style, not temp-dir paths.
- **Adding skills**: wire into `agentmd.rs::install_skill()`, register in `workspace.rs::check_claude_dir()` managed + managed_dirs sets. Run `/check-skill-registration` to verify.
- **Adding commands or output structs**: run `just skill` after. `just ci` fails if SKILL.md is stale.
- **CLI changes**: every new command needs `.about()` (short) and `.long_about()` (conceptual). Shell completers are mandatory for known-value args.
- **Adding features that touch invariants**: consider whether `wsp doctor` should validate it. Every Warn/Error check must be auto-fixable or include actionable guidance.
- **`help` subcommand**: custom implementation in `cli/help.rs`. Dispatches before `Paths::resolve()` so it works even with broken config.
- **Default dispatch** uses root-level `ArgMatches` — use `try_get_one().ok().flatten()` not `get_flag()`.
- **Interactive prompts**: gate on `stdin().is_terminal()`. EOF returns `""`, Enter returns `"\n"` — detect EOF before trimming.
- **Config key naming**: dot-separated groups (`git.<key>`, `lang.<name>`, `shell.tmux`). Old names normalized via `normalize_key()` with deprecation warning.
- **Workspace root skip list**: `check_root_content()` hardcodes `.wsp.yaml`, `.wsp.yaml.lock`, `.wspignore`, repo dirs. Add new wsp-managed root files here.

## Releasing

See `/wsp-release` skill for the multi-step process. Key gotcha: dry-run modifies `CHANGELOG.md` — run `git checkout CHANGELOG.md` before executing.
