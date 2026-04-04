# Changelog

All notable changes to this project will be documented in this file.

## [0.15.0] - 2026-04-04

### Features

- *(setup)* Use gh login as default branch prefix suggestion
- *(workspace)* Add -b flag to wsp new and wsp add for existing remote branches
- *(workspace)* Add setup_commands with hash-based approval flow (#32)
- *(rename)* Resolve '.' to current workspace (#42)
- *(hints)* Add git-style advice.* contextual hint system (#44)
- *(init)* Add wsp init to scaffold per-repo .wsp.yaml (#43)
- *(new)* Add --empty flag (#46)
- *(init)* Add .wsp.yaml.lock to .gitignore
- *(setup-commands)* Multi-layer setup command management
- *(hints)* Add registrySetupCommands hint; add dedup and runner tests
- *(setup-commands)* Prompt before clear; require --yes for scripts
- *(hints)* Add per-day cooldown and state-driven setupCommands suppress
- *(new)* Auto-track existing remote branch when computed name matches
- *(whatsnew)* Add wsp whatsnew command and upgrade notice
- *(new,add)* Unified per-repo branch tracking with mixed summary
- *(pr)* Add PR awareness to wsp st and wsp rm

### Bug Fixes

- *(security)* Strip credentials from HTTPS URLs before persisting to config
- *(security)* Print full agent_md content before writing to AGENTS.md
- *(doctor)* Include repo identities in template-repos-registered messages
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
- *(release)* Fix CHANGELOG path for virtual workspace crate layout
- *(whatsnew)* Remove em dash; fix auto-track to require all mirrors
- *(new)* Auto-track with .any(); skip full-mirror validation for auto-detected branches
- *(new)* Remove branch pre-flight check; track per-repo where available
- *(new)* Auto-track remote branch when computed name matches
- *(recover)* Add shell completion for workspace names

### Refactor

- *(wsp-core)* Narrow public API surface
- *(rm)* Drop --permanent flag
- *(test)* Clean up reviewer feedback
- *(setup)* Remove --force flag from wsp repo setup

### Testing

- Add meaningful coverage for status, completers, and sync
- *(gc)* Add integration tests for gc warning on stderr

### Miscellaneous

- Move source tree into crates/ workspace structure


