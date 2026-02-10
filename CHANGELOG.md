# Changelog

All notable changes to this project will be documented in this file.

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


