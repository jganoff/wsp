# ws - Multi-Repo Workspace Manager

## Build & Test

- `cargo build --release` - Build optimized binary
- `cargo test` - Run all tests
- `cargo clippy -- -D warnings` - Run linter
- `cargo fmt` - Format code

## Architecture

- `src/main.rs` - Entry point with signal handling
- `src/cli/` - Clap command definitions
- `src/config.rs` - Config loading/saving, XDG paths
- `src/git.rs` - Git command execution wrapper
- `src/giturl.rs` - URL parsing and shortname resolution
- `src/mirror.rs` - Bare clone management
- `src/workspace.rs` - Workspace CRUD and worktree ops
- `src/group.rs` - Group management
- `src/output.rs` - Table formatting and status display

## Data Storage

- Config: `~/.local/share/ws/config.yaml`
- Mirrors: `~/.local/share/ws/mirrors/<host>/<user>/<repo>.git/`
- Workspaces: `~/dev/workspaces/<name>/` with `.ws.yaml` metadata

## Conventions

- Git ops via `std::process::Command`, not libgit2
- Table-driven tests
- YAML config with `serde_yml`
- Error handling with `anyhow`
