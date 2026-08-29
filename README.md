# wsp

**Ship cross-repo changes faster.**

`wsp` gives every feature or fix one isolated workspace across all the
repositories it touches, so you can start coding immediately, see the whole
change at once, and clean up safely when it ships.

```text
~/dev/workspaces/fix-build/
├── compose/    # branch: yourname/fix-build
└── buildx/     # branch: yourname/fix-build
```

Use `wsp` when one change spans several repositories, when you need multiple
features checked out at once, or when you want to give a coding agent clean,
self-contained context.

- **Start fast.** Repositories are cloned from local mirrors, so new workspaces
  are quick and can be created offline after the first fetch.
- **Work normally.** Every repository is a standard Git clone with its usual
  `origin`. Use Git, your editor, and your existing tools as before.
- **See the whole change.** Short commands show status and diffs or run a
  command across every repository.
- **Clean up safely.** `wsp rm` protects uncommitted and unmerged work, and
  removed workspaces can be recovered.

## Get started

Install `wsp`:

**macOS or Linux**

```bash
brew install jganoff/tap/wsp
```

**Windows (PowerShell)**

```powershell
irm https://github.com/jganoff/wsp/releases/latest/download/wsp-installer.ps1 | iex
```

Then run the guided one-time setup:

```bash
wsp setup
```

It checks that Git is available, configures your branch prefix, and offers to
enable tab completion and automatic directory changes when supported.

Register the repositories you work with once, then create a workspace:

```bash
wsp registry add https://github.com/docker/compose.git
wsp registry add https://github.com/docker/buildx.git

wsp new fix-build compose buildx
```

Your workspace is ready at `~/dev/workspaces/fix-build/`, with both repositories
on the same feature branch. If shell integration is active, `wsp new` takes you
there automatically. Otherwise, run:

```bash
wsp cd fix-build
```

## Your daily workflow

From anywhere inside a workspace, `wsp` detects the workspace automatically:

```bash
wsp st                       # see status across every repo
wsp diff                     # review the complete change
wsp sync                     # fetch and rebase every repo
wsp exec -- go test ./...    # run a command in every repo
```

Status stays readable even when the repositories do not agree:

```text
$ wsp st
Workspace: fix-build  Branch: yourname/fix-build

Repository  Branch                  Status
buildx      yourname/fix-build      1 ahead, 2 files changed
compose     yourname/fix-build      clean
```

Push and open pull requests with your normal Git workflow. When the work is
done, remove the workspace:

```bash
wsp rm fix-build
```

Removal is recoverable by default. `wsp` blocks removal when it finds
uncommitted work or unmerged branches, including squash-merged pull requests.
Use `wsp ls --removed` to see restorable workspaces and `wsp recover <name>` to
bring one back.

## Reuse a set of repositories

Save repositories as a template when you often work on them together:

```bash
wsp template new docker-dev compose buildx
wsp new my-feature -t docker-dev
```

Templates are shareable YAML files:

```bash
wsp template export docker-dev
wsp template import docker-dev.wsp.yaml
```

## Set up repositories after cloning

A repository can declare trusted post-clone setup commands in a `.wsp.yaml` at
its root:

```yaml
setup_commands:
  - task setup
  - lefthook install
```

When `wsp` creates a workspace or adds a repository, it displays these commands
and asks before running them. Approval is remembered and requested again if the
commands change.

```text
Setup commands for github.com/acme/api-gateway:
  task setup
  lefthook install
Run these commands? [y/N] y
```

Run them again with `wsp repo setup`. If a repository was cloned before its
commands were approved, `wsp doctor --fix` can finish the setup.

## Shell integration

`wsp setup` can configure shell integration for you. To configure it manually,
add the matching line to your shell's startup file:

```bash
# zsh
eval "$(wsp completion zsh)"

# bash
eval "$(wsp completion bash)"

# fish
wsp completion fish | source
```

This enables tab completion, moves into a workspace after `wsp new`, and moves
out safely when removing the workspace you are currently in.

## Command overview

| Goal | Command |
|------|---------|
| Create a workspace | `wsp new <name> [repos...] [-t template]` |
| List workspaces | `wsp ls` |
| Enter a workspace | `wsp cd <workspace>` |
| See status across repos | `wsp st [workspace]` |
| Review changes across repos | `wsp diff [workspace] [-- args]` |
| View history across repos | `wsp log [workspace] [-- args]` |
| Fetch and rebase repos | `wsp sync [workspace]` |
| Run a command across repos | `wsp exec [workspace] -- <command>` |
| Add or remove repos | `wsp repo add/rm` |
| Remove a workspace | `wsp rm [workspace]` |
| Recover a removed workspace | `wsp recover <workspace>` |
| Manage registered repos | `wsp registry add/ls/rm` |
| Manage templates | `wsp template new/import/ls/show/rm/export` |
| Manage settings | `wsp config ls/get/set/unset` |

Every command supports `--json` for scripts and coding agents. See the
[full usage guide](docs/usage.md) or run `wsp help <command>` for details.

## How it works

```text
~/.local/share/wsp/
└── mirrors/
    └── github.com/docker/
        ├── compose.git/     # bare mirror; fetched once
        └── buildx.git/

~/dev/workspaces/
└── fix-build/
    ├── .wsp.yaml            # workspace metadata
    ├── compose/             # normal local clone
    └── buildx/              # normal local clone
```

Registered repositories have a bare local mirror. Workspace clones reuse its
Git objects through local hardlinks, while each clone keeps a single `origin`
pointing to the real upstream. No `wsp`-specific remotes or Git configuration
leak into your repositories.

## Other installation methods

<details>
<summary>Download a binary</summary>

Download a prebuilt archive from the
[latest release](https://github.com/jganoff/wsp/releases/latest):

- **Windows:** `wsp-x86_64-pc-windows-msvc.zip` or
  `wsp-aarch64-pc-windows-msvc.zip`
- **macOS:** `wsp-aarch64-apple-darwin.tar.xz`
- **Linux:** `wsp-x86_64-unknown-linux-gnu.tar.xz` or
  `wsp-aarch64-unknown-linux-gnu.tar.xz`

Extract the archive and add the binary to your `PATH`.

</details>

<details>
<summary>Build from source</summary>

Requires [Rust](https://www.rust-lang.org/tools/install).

```bash
cargo install --git https://github.com/jganoff/wsp.git
```

From a local clone:

```bash
cargo install --path crates/wsp
```

</details>

## Contributing

Development requires [Rust](https://www.rust-lang.org/tools/install) (stable)
and [just](https://github.com/casey/just).

```bash
just setup    # install the pre-commit hook once
just          # format and lint checks
just build    # build a release binary
just test     # run all tests
just ci       # run the full CI pipeline
```

## License

[MIT](LICENSE)
