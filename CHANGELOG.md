# Changelog

All notable changes to this project will be documented in this file.

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


