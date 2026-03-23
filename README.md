# wsp

Multi-repo workspace manager. One command to create an isolated workspace
across multiple repositories, all on the same branch.

## Quick start

```bash
brew install jganoff/tap/wsp
wsp setup

# register repos once (creates local mirrors so future clones are instant)
wsp registry add https://github.com/docker/compose.git
wsp registry add https://github.com/docker/buildx.git

# create a workspace
wsp new fix-build compose buildx
```

That's it. You now have `~/dev/workspaces/fix-build/` with both repos cloned
on the `fix-build` branch. Jump in with `wsp cd fix-build`.

## Working in a workspace

```bash
wsp st                                # status across all repos
wsp diff                              # diff across all repos
wsp sync                              # fetch and rebase all repos
wsp exec fix-build -- go test ./...   # run a command in every repo
wsp rm fix-build                      # clean up when done
```

Status at a glance:

```
$ wsp st
Workspace: fix-build  Branch: fix-build

Repository  Branch     Status
buildx      fix-build  1 ahead, 2 files changed
compose     fix-build  clean
```

When you're ready to ship, it's just git — `git push` and open a PR like you
normally would. Or ask your agent to do it: "submit a PR for my changes" works
out of the box since each repo is a standard git clone.

`wsp rm` is safe — it blocks if any repo has uncommitted work or unmerged
branches (including squash-merged PRs). Removed workspaces are recoverable
via `wsp recover`.

## Shell integration

Tab completion and auto-cd after `wsp new`:

```bash
# zsh
eval "$(wsp completion zsh)"

# bash
eval "$(wsp completion bash)"

# fish
wsp completion fish | source
```

## Templates

Save a set of repos so you don't have to remember them every time:

```bash
wsp template new docker-dev compose buildx
wsp new my-feature -t docker-dev
```

Share templates by checking them into a repo or passing the file around:

```bash
wsp template export docker-dev       # writes docker-dev.wsp.yaml
wsp template import docker-dev.wsp.yaml   # import on another machine
```

## Per-repo setup

Repos can declare post-clone setup commands in their own `.wsp.yaml`:

```yaml
# checked into the repo root
setup_commands:
  - task setup
  - lefthook install
```

When you create or add a workspace, `wsp` shows the commands and asks for
approval before running anything. Answer `always` to remember the decision; it
re-prompts automatically if the command list changes upstream.

```
Setup commands for github.com/acme/api-gateway:
  task setup
  lefthook install
Run these commands? [y/always/N] always
```

Re-run at any time with `wsp repo setup`. `wsp doctor --fix` handles repos that
were cloned without approval.

## Configuration

Prepend your name to all workspace branches:

```bash
wsp config set branch-prefix myname
# wsp new fix-build → creates branch myname/fix-build
```

## Commands

**Workspace lifecycle:**

| Command | Description |
|---------|-------------|
| `wsp new <name> [repos...] [-t template]` | Create a workspace |
| `wsp rm [workspace] [-f]` | Remove (recoverable by default) |
| `wsp ls` | List workspaces |
| `wsp cd <workspace>` | Jump into a workspace |
| `wsp recover [workspace]` | Restore a removed workspace |
| `wsp rename <old> <new>` | Rename a workspace |

**Daily workflow:**

| Command | Description |
|---------|-------------|
| `wsp st [workspace]` | Git status across repos |
| `wsp diff [workspace] [-- args]` | Git diff across repos |
| `wsp log [workspace] [-- args]` | Git log across repos |
| `wsp sync [workspace]` | Fetch and rebase all repos |
| `wsp exec <workspace> -- <cmd>` | Run a command in each repo |

**Repo and admin:**

| Command | Description |
|---------|-------------|
| `wsp repo add/rm/ls/fetch` | Manage repos in current workspace |
| `wsp repo setup [repos] [--force]` | Run per-repo setup commands with approval prompt |
| `wsp registry add/ls/rm` | Manage registered repositories |
| `wsp template new/import/ls/show/rm/export` | Manage workspace templates |
| `wsp config ls/get/set/unset` | Manage settings |

All commands support `--json` for scripting and AI agents.
See [docs/usage.md](docs/usage.md) for the full reference.

## How it works

```
~/.local/share/wsp/
  mirrors/
    github.com/docker/
      compose.git/           bare mirror (one network clone)
      buildx.git/            bare mirror

~/dev/workspaces/
  fix-build/
    .wsp.yaml                workspace metadata
    compose/                 local clone (branch: fix-build)
    buildx/                  local clone (branch: fix-build)
```

Each repo is registered once as a bare mirror. Workspaces are directories of
local clones (via `git clone --local` hardlinks) that share a branch name.
Each clone has a single `origin` remote pointing to the real upstream — no
wsp-specific remotes or config leak into your repos.

## Other install methods

<details>
<summary>Binary download or build from source</summary>

Download a binary from the [latest release](https://github.com/jganoff/wsp/releases/latest), or build from source:

```
cargo install --git https://github.com/jganoff/wsp.git
```

To install from a local clone:

```
cargo install --path crates/wsp
```
</details>

## Development

Requires [Rust](https://www.rust-lang.org/tools/install) (stable) and
[just](https://github.com/casey/just).

```
just          # check (fmt + clippy)
just build    # build release binary
just test     # run all tests
just ci       # full CI pipeline
```

## License

[MIT](LICENSE)
