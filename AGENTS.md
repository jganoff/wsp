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
- **Workspace positional must be optional**: every command that operates on a single workspace takes `<workspace>` as an optional positional with CWD detection fallback via `workspace::detect()`. When a command also has a second positional (e.g. `describe [workspace] <text>`), keep both as named args, make the second optional in clap, and in `run()` treat the single-arg case as "detect workspace from CWD, first arg is the payload." Use `.override_usage()` to show the correct semantic order. See `describe.rs` and `rename.rs` for the pattern. A `test_workspace_arg_is_optional` test in `cli/mod.rs` enforces this.
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
- **`WHATSNEW.md` is embedded at compile time** alongside `CHANGELOG.md` in `whatsnew.rs`. Uses the same `## [X.Y.Z] - date` header format. `wsp whatsnew` tries `WHATSNEW.md` first, falls back to `CHANGELOG.md`. The release skill generates prose notes and prepends them to `WHATSNEW.md` before executing the release.
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
- **`rust-toolchain.toml` pins an exact version**, not `stable`. When the nightly `toolchain` job goes red (`pins X but stable is now Y`), bump it in its own commit and fix any new clippy lints there — that is the whole point of the check, keeping new lints out of unrelated PRs. Run `rustup update stable` first so local matches CI; a stale local `stable` is what made 1.96 pass locally while 1.97 failed in CI. Keep `components = ["clippy", "rustfmt"]`: rustup auto-installs the pin using the machine's profile, which is `default` locally but `minimal` on runners, where `cargo fmt` then fails with "'cargo-fmt' is not installed for the toolchain".
- **Scheduled workflows in `audit.yml`** (`audit`, `toolchain`, `dormancy`) do not run on pull requests — the workflow is `schedule` + `workflow_dispatch` only. A PR touching it proves nothing; verify with `gh workflow run audit.yml` after merging. Failing loudly is the delivery mechanism for all three, since GitHub notifies whoever last edited the cron syntax.

## Releasing

See `/wsp-release` skill for the multi-step process. Key gotcha: dry-run modifies `CHANGELOG.md` — run `git checkout CHANGELOG.md` before executing.

**Do not manually edit `release.yml`.** It is fully owned by `cargo-dist` — any hand-edits (e.g. SHA-pinning actions) will be overwritten the next time `dist generate` runs, and the `dist plan` freshness check in CI will fail on every PR until they are reverted.

**To update the actions in `release.yml`, bump cargo-dist** in `dist-workspace.toml` and re-run `dist generate`. That is the only supported path; Dependabot cannot do it (see below).

**Dependabot PRs that touch `release.yml` cannot be merged — close them.** Dependabot cannot be scoped to a subset of workflow files, so its grouped actions PR always includes `release.yml`, always fails `plan`, and is blocked. Those hunks are futile regardless: the next `dist generate` overwrites them. Take the `ci.yml` / `audit.yml` changes as your own PR and drop the rest.

This gap is inherent, not version lag — dist pins what it pins while Dependabot chases latest, so no cargo-dist version closes it (0.32.0 emits `checkout@v6` against Dependabot's `v7`). It only recurs when an action ships a new major, since closing a Dependabot PR suppresses that version until a newer one appears.

Note Dependabot leaves the trailing `# v4`-style comment stale when it bumps a SHA pin — it will happily put `# v4` beside a v7.0.1 SHA. For a SHA pin that comment is the only human-readable version marker, so correct it by hand; a wrong one is worse than none.

### Prereleases

Tag with a prerelease suffix (`v0.19.0-prerelease.1`) and `release.yml` marks the GitHub release as a prerelease automatically. Artifacts are built for every target, but **the homebrew tap is deliberately left alone**, so `brew install` users are unaffected. Testers opt in by installing that specific release.

**Do not set `publish-prereleases = true` to get prereleases into homebrew.** cargo-dist accepts the key, but `formula` is a single value — the prerelease would overwrite the *stable* `wsp` formula and ship to everyone on their next `brew upgrade`, which is the opposite of what a prerelease channel is for. A real homebrew beta channel needs a second formula (`wsp-beta`) published by a hand-written job, with `conflicts_with` since both install a `wsp` binary. Deliberately not built — testers use the installer or download artifacts instead.
