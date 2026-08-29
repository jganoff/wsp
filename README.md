# wsp

**Ship cross-repo changes faster.**

`wsp` gives developers and coding agents one isolated workspace containing
every repository a feature or fix touches. Start with complete context, see the
whole change at once, and clean up safely when it ships.

```text
~/dev/workspaces/fix-build/
├── api/    # branch: yourname/fix-build
└── web/    # branch: yourname/fix-build
```

- **Start fast.** Repositories are cloned from local mirrors, so new workspaces
  are quick and can be created offline after the first fetch.
- **Give agents complete context.** Every workspace tells coding agents which
  repos belong together, where they live, what branch to use, and how to operate
  across them.
- **Work normally.** Every repository is a standard Git clone with its usual
  `origin`. Use Git, your editor, and your existing tools as before.
- **See the whole change.** Short commands show status and diffs or run a
  command across every repository.
- **Clean up safely.** `wsp rm` protects uncommitted and unmerged work, and
  removed workspaces can be recovered.

## Get started

`wsp` requires Git. Install it with [Homebrew](https://brew.sh/) on macOS or
Linux, or with the PowerShell installer on Windows:

**macOS or Linux**

```bash
brew install jganoff/tap/wsp
```

**Windows (PowerShell)**

```powershell
irm https://github.com/jganoff/wsp/releases/latest/download/wsp-installer.ps1 | iex
```

Verify the install, then run the guided one-time setup:

```bash
wsp --version
wsp setup
```

Setup checks Git and configures your branch prefix. Now register the `wsp`
repository and create a workspace for your first change:

```bash
wsp registry add https://github.com/jganoff/wsp.git
wsp new improve-readme wsp
```

You now have an isolated clone of `wsp` on the `yourname/improve-readme` branch.
Open the printed workspace path in your editor and make a change to
`wsp/README.md`, then review it from anywhere:

```bash
wsp st improve-readme
wsp diff improve-readme
```

The first registration downloads a local mirror; future workspaces reuse it.
Add more repositories to the same workspace whenever a change spans repo
boundaries.

`wsp new` prints the workspace path. With shell integration active, it also
takes you there automatically. Otherwise, use your shell's `cd` command with
the printed path.

## Your daily workflow

From anywhere inside a workspace, `wsp` detects the workspace automatically:

```bash
wsp st                       # see status across every repo
wsp diff                     # review the complete change
wsp sync                     # fetch and rebase every repo
wsp exec -- git status --short --branch  # run a command in every repo
```

Status stays readable even when the repositories do not agree:

```text
$ wsp st
Workspace: improve-readme  Branch: yourname/improve-readme

Repository  Branch                      Status
wsp         yourname/improve-readme     1 file changed
```

Push and open pull requests with your normal Git workflow. When the work is
done, remove the workspace:

```bash
wsp rm improve-readme
```

Removal is recoverable by default. `wsp` blocks removal when it finds
uncommitted work or unmerged branches, including squash-merged pull requests.
Use `wsp ls --removed` to see restorable workspaces and `wsp recover <name>` to
bring one back.

## Built for coding agents

A coding agent is only as effective as the context it can see. Launch one at
the workspace root and it can work across the complete change instead of
discovering one repository at a time or editing the wrong checkout.

Each workspace includes a generated `AGENTS.md` that identifies the workspace
branch, maps repository names to directories, defines safe workspace
boundaries, and points agents to per-repo instructions. `wsp` also installs a
workspace-management skill so supported agents can discover its commands
without being prompted through the workflow.

Every `wsp` command supports `--json`, giving agents structured workspace and
Git state instead of making them scrape terminal output. The repositories
remain normal clones, so agents use the same build tools, tests, and Git
workflow as developers.

## Reuse a set of repositories

Save repositories as a template when you often work on them together:

```bash
wsp template new product-dev api web
wsp new my-feature -t product-dev
```

Templates are shareable YAML files:

```bash
wsp template export product-dev
wsp template import product-dev.wsp.yaml
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

```powershell
# PowerShell
Invoke-Expression (wsp completion powershell | Out-String)
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
    └── github.com/jganoff/
        └── wsp.git/         # bare mirror; fetched once

~/dev/workspaces/
└── improve-readme/
    ├── .wsp.yaml            # workspace metadata
    └── wsp/                 # normal local clone
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
