# Changelog

All notable changes to this project will be documented in this file.

## [0.1.1] - 2026-02-10

### Features

- *(workspace)* Auto-delete merged branches on ws remove
- *(completion)* Add dynamic shell completions via clap CompleteEnv
- *(cli)* Add ws diff subcommand
- *(completion)* Add dynamic shell completions via clap CompleteEnv
- *(config)* Add branch prefix for workspace branches
- *(cli)* Add --json output and Claude Code skill
- *(lang)* Add go.work auto-generation for multi-repo workspaces

### Bug Fixes

- *(completion)* Resolve workspaces dir from config instead of hardcoding
- *(git)* Configure fetch refspec for bare mirror clones

### Refactor

- Inject path resolution to eliminate env var mutation in tests

### Miscellaneous

- *(docs)* Remove obsolete Go-era output formatting design doc
- Apply cargo fmt


