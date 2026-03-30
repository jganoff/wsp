# Changelog

All notable changes to this project will be documented in this file.

## [0.14.0] - 2026-03-30

### Miscellaneous

- Update dependencies to latest compatible versions

## [0.14.0] - 2026-03-30

### Features

- *(setup)* Use gh login as default branch prefix suggestion
- *(wsp-core)* Publish as embeddable library
- *(workspace)* Add -b flag to wsp new and wsp add for existing remote branches
- *(workspace)* Add setup_commands with hash-based approval flow (#32)
- *(rename)* Resolve '.' to current workspace (#42)
- *(hints)* Add git-style advice.* contextual hint system (#44)
- *(init)* Add wsp init to scaffold per-repo .wsp.yaml (#43)
- *(new)* Add --empty flag (#46)
- *(init)* Add .wsp.yaml with setup commands
- *(init)* Add .wsp.yaml.lock to .gitignore
- *(init)* Scaffold per-repo .wsp.yaml; ensure lock file is in .gitignore
- *(setup-commands)* Multi-layer setup command management
- *(hints)* Add registrySetupCommands hint; add dedup and runner tests
- *(setup-commands)* Prompt before clear; require --yes for scripts
- *(hints)* Add per-day cooldown and state-driven setupCommands suppress

### Bug Fixes

- *(security)* Strip credentials from HTTPS URLs before persisting to config
- *(security)* Print full agent_md content before writing to AGENTS.md
- *(config)* Case-insensitive false comparison and panic on missing parent
- *(workspace)* Several correctness and robustness fixes
- *(gc)* Use symlink_dir for directory symlinks on Windows; test cooldown expiry
- *(security)* Unify git config denylist and harden template apply
- *(workspace)* Replace silent empty path fallback with expect
- *(doctor)* Include repo identities in template-repos-registered messages
- *(workspace)* Check linked worktrees during removal safety checks
- *(workspace)* Block removal when clean external linked worktrees exist
- *(ci)* Fix several pre-existing CI failures on jganoff/wsp-ci branch (#31)
- *(workspace)* Don't block removal on squash-merged branch when remote tracking deleted (#33)
- *(hints)* Add blank line before contextual hints (#48)
- *(detect)* Skip per-repo .wsp.yaml during workspace detection (#49)
- *(template)* Resolve repo shortnames in template new and repo add
- *(template)* Surface resolve error for ambiguous/not-found shortnames
- *(completion)* Remove leading comment that breaks eval $(...) without quotes (#52)
- *(setup)* Correct post-setup guide to use wsp registry add (#55)
- *(sync)* Skip repos on wrong branch instead of erroring
- *(repo)* Checkout specified branch when using repo@branch syntax
- *(rm)* Handle partial workspace (no .wsp.yaml) in rm and recover
- *(new)* Include <name> in branch-not-found hint
- *(wsp-report)* Redact personal details; drop registry from default gather

### Refactor

- *(wsp-core)* Narrow public API surface
- *(rm)* Drop --permanent flag
- *(test)* Clean up reviewer feedback
- *(setup)* Remove --force flag from wsp repo setup

### Documentation

- Mention wsp setup in quick start
- Slim AGENTS.md to routing doc, extract to dedicated files
- *(claude)* Note workspace::remove caller locations in gotchas
- *(git)* Clarify resolve_upstream_ref resolution order and Head rarity
- *(conventions)* Document --force vs --yes semantic distinction

### Testing

- Add shared test helpers and concurrent FileLock tests
- Add meaningful coverage for status, completers, and sync
- *(git)* Document UpstreamRef::Head false-clean behavior
- *(gc)* Add integration tests for gc warning on stderr
- *(gc)* Add coverage for cross-filesystem copy fallback

### Miscellaneous

- *(infra)* Pin release.yml actions to SHA, add rust-toolchain.toml, tidy config
- Remove dead build.rs at workspace root
- Move source tree into crates/ workspace structure
- *(roadmap)* Migrate roadmap to GitHub issues (#45)

## [0.13.2] - 2026-03-16

### Features

- *(gc)* Prominent banner warning for removed workspaces

### Bug Fixes

- *(recover)* Cd into restored workspace after wsp recover <name>
- *(gc)* Eliminate nested format! in row closure to satisfy clippy

### Miscellaneous

- Fix rustfmt formatting in completion and gc

## [0.13.1] - 2026-03-14

### Features

- *(doctor)* Add git config drift detection (W14)

### Bug Fixes

- *(workspace)* Register wsp-new-feature in check_claude_dir managed sets

## [0.13.0] - 2026-03-13

### Bug Fixes

- *(tmux)* Only rename window from active pane, respect user-set titles

### Refactor

- *(config)* Normalize key naming to dot-separated groups

## [0.12.0] - 2026-03-13

### Features

- *(config)* Add workspace-scoped config overrides
- *(exec)* Make workspace argument optional via cwd detection

### Bug Fixes

- *(agentmd)* Strengthen workspace root boundary in AGENTS.md

## [0.11.0] - 2026-03-12

### Features

- *(skill)* Add wsp-new-feature skill for creating feature workspaces
- *(cli)* Add `wsp setup` interactive onboarding wizard

### Bug Fixes

- *(setup)* Exit immediately on Ctrl-C, check all shell rc files
- *(setup)* Remove SSH/HTTPS prompt, default to SSH

### Refactor

- *(setup)* Replace repo import with "what's next" guide

## [0.10.0] - 2026-03-12

### Features

- *(template)* Add rename command and fix group migration
- Remove deprecated group feature [**breaking**]
- *(doctor)* Add diagnostic command for workspace and global state
- *(doctor)* Expand check catalog with 11 new diagnostics
- *(doctor)* Add remaining checks (G3, G5-G8, W5-W6, W10-W11, W13)
- *(config)* Add experimental features gate
- *(completion)* Add experimental shell hooks for tmux title and prompt
- *(config)* Add shell completions for config keys and values
- *(config)* Add post-set hints and doctor check for unknown experimental keys
- *(recover)* Add show command, expiration info, and gc improvements
- *(completion)* Add shell completions for help topics and commands
- *(cli)* Default to ls when subcommand group is called bare
- *(new)* Implicitly copy repos from current workspace
- *(output)* ISO timestamps in log JSON and relative time in status
- *(output)* Structured mutations and absolute paths in samples
- *(agentmd)* Add per-repo conventions and feedback loop sections
- *(shell)* Refactor tmux integration to use rename-window

### Bug Fixes

- *(workspace)* Handle empty repos in clone_from_mirror
- *(doctor)* Address code review findings for Phase 3 checks
- *(completion)* Guard against missing compinit in zsh
- *(completion)* Hide experimental feature flags when gate is off
- *(doctor)* Enforce "warnings must have fixes" design rule
- Make wsp new and repo rm idempotent

### Refactor

- *(output)* Rename wrong_branch to expected_branch in JSON
- *(output)* Standardize repo field names and add path to JSON
- *(output)* Add workspace context to JSON and rename top-level keys
- *(cli)* Remove deprecated wsp setup command and -t file paths

### Documentation

- Remove stale -g/--group reference from templates design doc
- Add compdef guard and CLAUDE.md symlink notes to CLAUDE.md
- *(help)* Add config guide with all keys and defaults
- *(diff)* Document git arg passthrough with examples
- Add output struct gotcha to CLAUDE.md
- Add shell startup resilience convention to CLAUDE.md
- Add "operations are resumable" safety tenet, remove transaction journal

### Testing

- *(doctor)* Comprehensive coverage for all check detection and fix paths

## [0.9.2] - 2026-03-09

### Bug Fixes

- Gate unix symlink code with cfg and add cross-compile CI check

## [0.9.1] - 2026-03-09

### Features

- Add YAML size cap, git-ref validation, and CreateInnerOpts refactor
- *(help)* Add `wsp help` command with concept guides

### Bug Fixes

- Tech debt and security cleanup across codebase
- *(fetch)* Preserve panic message in thread join error

## [0.9.0] - 2026-03-09

### Features

- *(config)* Add git config defaults for workspace clones
- *(workspace)* Add .wspignore for suppressing root content warnings
- *(template)* Show customizations when importing external templates
- *(agentmd)* Add workspace boundary directive to generated AGENTS.md
- *(template)* Add mutation subcommands for repos, config, and agent-md
- *(agentmd)* Add multi-repo guidance preamble to generated AGENTS.md
- *(template)* Add sharing via repo-checked-in files

### Bug Fixes

- *(workspace)* Narrow default wspignore to only .claude/settings.local.json
- *(filelock)* Add advisory locking for template and rename mutations

### Documentation

- Update file locking convention to include with_template()
- Streamline README for scannability
- Update README with Docker examples and copy-pasteable quickstart

### Miscellaneous

- *(discovery)* Remove unused DiscoveredTemplateOutput scaffolding

## [0.8.0] - 2026-03-08

### Features

- *(workspace)* Adopt existing git directories in `wsp repo add`
- *(agentmd)* Add troubleshooting section and wsp-report skill
- *(gc)* Deferred deletion for wsp rm with recovery
- Add wsp rename command
- *(cli)* Categorized help output with command grouping
- *(new)* Show elapsed time after workspace creation
- *(st)* Enrich status with agent context
- *(sync)* Add --abort to abort in-progress rebase/merge
- *(workspace)* Add descriptions and age/staleness signals
- *(st)* Show workspace age in status header
- *(template)* Add workspace templates phase 1
- *(template)* Polymorphic --from and export (phase 2)
- *(template)* Migrate groups to templates (phase 3)
- *(template)* Wire template settings into workspace creation (phase 4)
- *(template)* Unify .wsp.yaml as template format (phase 5)
- *(template)* Inline agent_md content in templates (phase 6)
- *(ls)* Add sorting options to wsp ls
- *(new)* Add -w/--workspace and -f/--file flags for wsp new

### Bug Fixes

- *(workspace)* Fast-forward local default branch after clone
- *(cli)* Normalize verb naming across setup subcommands
- *(mirror)* Keep refs/heads in sync to prevent dirty index on wsp new
- *(status)* Add wsp-report skill to managed paths in check_claude_dir
- *(shell)* Cd out of deleted workspace on wsp rm with flags
- *(status)* Detect wrong-branch in wsp st and wsp rm
- *(workspace)* Skip .wsp.yaml.lock in root content check
- *(new)* Validate workspace name before fetching mirrors
- *(st)* Show 'wsp st -v' instead of '-v' in file details hint
- *(template)* Add shell completion for --from flag
- *(template)* Auto-migrate groups on template commands too
- *(template)* Address code review findings
- *(template)* Reject marker injection and warn on external agent_md
- *(ci)* Fix shell variable escaping in manpage freshness check
- *(gc)* Warn when running commands inside GC'd workspaces

### Refactor

- *(skill)* Install all skills from single SKILLS array
- *(cli)* Drop hidden delete alias for group rm
- *(gc)* Move gc dir to ~/.local/share/wsp/gc/
- *(cli)* Flatten setup into top-level nouns
- *(cli)* Group help by workflow stage
- Remove context repos
- *(workspace)* Remove last_used tracking, simplify to created-only age
- *(template)* Rename settings to config for consistency

### Documentation

- Add CLI restructure design, rename definitions to templates
- Add "don't duplicate unix" tenet, move staleness to P1
- *(roadmap)* Add git config defaults for workspace clones
- Remove skill subcommand from CLI restructure plan
- Add safety tenets for data-loss prevention
- *(roadmap)* Remove completed items, consolidate gc into doctor
- Regenerate SKILL.md for describe command
- Add completions convention and metadata gotcha to AGENTS.md
- *(templates)* Add agent context and repo-embedded template roadmap items
- Update roadmap for phase 3 shipped
- *(templates)* Unify .wsp.yaml as the template format
- Add WorkspaceRepoRef field gotcha to AGENTS.md
- *(justfile)* Add comment about shell variable escaping
- *(cli)* Add long_about descriptions to all commands

### Testing

- *(sync)* Add tests for behind_count, in_progress_op, abort, exit_code

### Miscellaneous

- Remove repo-adopt design doc (feature is implemented)
- Regenerate SKILL.md and manpages for template commands

## [0.7.0] - 2026-03-06

### Features

- *(go)* Discover nested go.mod files in repo trees

### Bug Fixes

- *(mirror)* Fetch after bare clone to populate remote-tracking refs
- *(lang)* Make go workspace integration opt-in

### Documentation

- Add workspace definitions design doc and roadmap entry

## [0.6.0] - 2026-03-05

### Features

- *(workspace)* Remove wsp-mirror remote, route all fetches through mirrors
- *(filelock)* Add advisory file locking for concurrent write safety
- *(exec)* Add --json output for structured per-repo results

### Bug Fixes

- *(workspace)* Detect intra-batch dir name collisions in add_repos

### Documentation

- *(roadmap)* Remove git subprocess timeouts from roadmap
- Add design tenets for git/mirror, agent, and human use
- *(roadmap)* Add P0 for removing wsp-mirror remote from clones
- *(roadmap)* Expand roadmap from multi-perspective analysis
- *(roadmap)* Remove completed P0, reorder remaining items
- *(roadmap)* Remove completed file locking item

## [0.5.5] - 2026-03-03

### Features

- *(repo)* Add --from flag for bulk GitHub org import

### Bug Fixes

- *(agentmd)* Use platform-gated symlink for Windows compatibility

## [0.5.4] - 2026-03-02

### Features

- *(cli)* Auto-generate SKILL.md from clap introspection
- *(config)* Add version field to Config and Metadata structs
- *(agentmd)* Generate AGENTS.md, CLAUDE.md symlink, and workspace skill
- *(status)* Add verbose file lists and workspace root visibility

### Refactor

- *(cli)* Remove `wsp push` command and drop `wsp pr` from roadmap
- Expose marker and header constants as pub(crate)

### Documentation

- Reprioritize roadmap based on strategic analysis
- *(roadmap)* Add .wspignore feature

## [0.5.3] - 2026-02-27

### Features

- *(cli)* Add `wsp log` command for cross-repo commit log
- *(cli)* Add `wsp sync` command for fetch + rebase/merge
- *(cli)* Add `wsp push` command with RepoInfo consolidation

### Bug Fixes

- *(workspace)* Default branch tracks origin instead of wsp-mirror

### Refactor

- *(workspace)* Consolidate removal safety checks and improve fetch accuracy

### Documentation

- Add AGENTS.md feature spec and expand roadmap
- Document removal safety checks and expected workflow
- Update roadmap after shipping sync/push/log

## [0.5.2] - 2026-02-18

### Bug Fixes

- *(workspace)* Validate dirs map on metadata load to prevent path traversal
- *(cli)* Fall back to identity when shortname lookup misses
- *(workspace)* Stop setting upstream tracking on new branches

### Documentation

- Fix inconsistent bullet formatting in "why wsp?" section
- Add feature roadmap with prioritized plan

## [0.5.1] - 2026-02-13

### Features

- *(cli)* Add `wsp repo ls` to list workspace repos

### Bug Fixes

- *(ci)* Use rustsec/audit-check action, add Rust dependency caching

### Documentation

- Add clap alias dispatch convention, update command list

## [0.5.0] - 2026-02-13

### Bug Fixes

- *(completion)* Escape single quotes in generated shell scripts
- *(release)* Add homebrew publish job to release workflow

### Refactor

- Rename all remaining ws references to wsp

### Documentation

- Rename heading references from ws to wsp in README
- Add naming conventions to CLAUDE.md

## [0.4.0] - 2026-02-13

### Bug Fixes

- *(build)* Rename crate to wsp, migrate to serde_yaml_ng, add cargo audit

### Refactor

- Rename CLI and all references from ws to wsp

## [0.3.3] - 2026-02-13

### Documentation

- Add release dry-run caveat and changelog recipe to CLAUDE.md
- Clarify dist init must regenerate release workflow

### Build

- Regenerate release workflow with Homebrew publish job

## [0.3.2] - 2026-02-13

### Documentation

- Recommend Homebrew install in README

### Miscellaneous

- Fix formatting in giturl.rs

## [0.3.1] - 2026-02-13

### Build

- Add Homebrew tap and Windows ARM64, drop Intel Mac

## [0.3.0] - 2026-02-13

### Features

- *(cli)* Default to status/list and add ws cd command
- *(cli)* Smart upstream detection for diff/status
- *(group)* Add ws group update command to add/remove repos
- *(config)* Add configurable workspaces-dir override
- *(workspace)* Auto-disambiguate worktree dirs for same-named repos
- *(cli)* Move fetch to daily ops, parallelize, and make prune opt-in
- *(workspace)* Detect squash-merged and pushed-to-remote branches in ws rm
- *(workspace)* Migrate from git worktrees to local clones
- *(completion)* Add bash and fish shell integration
- *(workspace)* Fetch origin after clone setup
- *(cli)* Show git describe in version for dev builds

### Bug Fixes

- *(config)* Use atomic write-then-rename for config and metadata saves
- *(config)* Show resolved workspaces-dir in config list/get
- *(go)* Preserve patch version in go.work generation
- *(diff)* Use merge-base to exclude unrelated upstream changes
- *(workspace)* Compare against origin/<default> for unmerged branch check
- *(completion)* Use context-aware completers for group update and repo rm
- *(git)* Add content-based squash-merge detection for diverged branches
- *(diff)* Enable colored output when stdout is a terminal
- *(git)* Track origin instead of ws-mirror for branch upstream
- *(completion)* Prevent shell injection via workspaces-dir config
- *(giturl)* Reject path traversal in identity components
- *(workspace)* Reject dot-prefixed workspace names

### Refactor

- *(cli)* Restructure daily ops vs setup administration

### Documentation

- Rewrite README for public release, move reference to docs/
- Rewrite README for easier onboarding
- Add tty color pattern and build.rs note to CLAUDE.md
- Update usage.md, SKILL.md, remove stale plan
- Replace personal name with generic in examples
- Add CLI command structure to CLAUDE.md

### Performance

- *(status)* Resolve upstream ref once per repo instead of twice

### Miscellaneous

- Add MIT license
- Apply cargo fmt
- Remove dead code (status, to_ssh_url, identity_to_ssh_url)
- CI hardening and misc cleanup

### Build

- Add Justfile and git pre-commit hook
- Add release and changelog targets to Justfile

## [0.2.0] - 2026-02-10

### Features

- Initial implementation of ws multi-repo workspace manager
- *(workspace)* Auto-delete merged branches on ws remove
- *(completion)* Add dynamic shell completions via clap CompleteEnv
- *(cli)* Add ws diff subcommand
- *(completion)* Add dynamic shell completions via clap CompleteEnv
- *(config)* Add branch prefix for workspace branches
- *(cli)* Add --json output and Claude Code skill
- *(lang)* Add go.work auto-generation for multi-repo workspaces
- *(release)* Add versioning and release automation pipeline

### Bug Fixes

- *(completion)* Resolve workspaces dir from config instead of hardcoding
- *(git)* Configure fetch refspec for bare mirror clones

### Refactor

- Apply idiomatic Rust cleanup from code review
- Inject path resolution to eliminate env var mutation in tests

### Miscellaneous

- *(docs)* Remove obsolete Go-era output formatting design doc
- Apply cargo fmt


