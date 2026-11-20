# wsp

**A fresh workspace for every task. One repo or many.**

Start a task without disturbing the work you already have going. Everything it
touches lives together, so you can review, test, and ship it as a unit.

```bash
wsp new refund-flow orders payments dashboard   # name the repos once
wsp new fix-retry                               # a second workspace, same repos
```

- **Run several agents at once.** Each task gets its own workspace, so agents
  don't step on each other or on you. Every workspace comes with instructions
  that teach agents how it's laid out and how to work inside it.
- **Works with your existing tools.** Every repository is an ordinary Git clone,
  so Git, your editor, and your build tools work unchanged.

## Get started

`wsp` requires Git 2.41 or newer. Install it and set it up once:

**macOS or Linux**

```bash
brew install jganoff/tap/wsp
wsp setup
```

**Windows (PowerShell)**

```powershell
irm https://github.com/jganoff/wsp/releases/latest/download/wsp-installer.ps1 | iex
wsp config set branch-prefix YOUR-USERNAME
```

Register the repositories you work on, once. After that, every new workspace is
fast to create:

```bash
wsp registry add https://github.com/acme/orders.git
wsp registry add https://github.com/acme/payments.git
wsp registry add https://github.com/acme/dashboard.git
```

Now start a feature that spans all three:

```bash
wsp new refund-flow orders payments dashboard
```

That is a directory with a normal clone of each repository, all on a
`yourname/refund-flow` branch. Open it in your editor or hand it to a coding
agent. Any checkout you already had stays exactly as you left it.

## Found a bug? Don't stop.

Halfway through `refund-flow`, you spot a bug in `payments`. Don't stash and
don't switch branches. From inside `refund-flow`, start a second workspace for
the fix:

```bash
wsp new fix-retry
```

It starts with the same repositories, and both workspaces now exist side by
side:

```text
$ wsp ls
NAME         BRANCH                REPOS  CREATED  DESCRIPTION
fix-retry    yourname/fix-retry    3      8m ago
refund-flow  yourname/refund-flow  3      3h ago
```

`wsp st` shows the whole feature at once. `payments` is clean in this
workspace, because the bug fix is in `fix-retry`:

```text
$ wsp st refund-flow
Workspace:   refund-flow
Branch:      yourname/refund-flow
Created:     2026-11-15 18:35 (3h ago)
Description: (none)

REPOSITORY  BRANCH                STATUS
dashboard   yourname/refund-flow  2 modified
orders      yourname/refund-flow  1 modified
payments    yourname/refund-flow  clean
```

From anywhere inside a workspace, `wsp` detects it, so you can leave the name
off:

```bash
wsp diff                  # review the whole change
wsp sync                  # fetch and rebase every repo
wsp exec -- make test     # run a command in every repo
```

Push each repository and open pull requests with your normal Git workflow.

When the work is done, `wsp rm fix-retry` removes the workspace. It refuses if
anything would be lost.

## Built for coding agents

While you stay in `refund-flow`, hand `fix-retry` to a coding agent started at
its workspace root. Each workspace has its own clones and branch, so the two
never overwrite each other's files, and either one sees all three repositories
in one place.

Each workspace includes a generated `AGENTS.md` that identifies the workspace
branch, maps repository names to directories, defines safe workspace
boundaries, and points agents to per-repo instructions. `wsp` also installs
skills so agents know which commands exist.

## Why not git worktrees?

If a task stays inside one repository, worktrees work fine. `wsp` is for tasks
that span several: one workspace holds every repository the task touches, so
status, diffs, and commands cover the whole change at once.

Worktrees also share state with the repository they came from. Hooks,
configuration, and branches live in one `.git` directory outside the worktree,
so an agent confined to the worktree can't commit without access to it, and
anything the agent changes there, such as a hook or a config setting, applies to
your other checkouts too. Each repository in a `wsp` workspace is an independent
clone with its own `.git` directory, so the workspace directory is the whole
boundary.

## Safety and trust

**What `wsp` changes on your machine.** It keeps its configuration, bare
mirrors, and removed workspaces under `~/.local/share/wsp`, and creates
workspaces in `~/dev/workspaces` (configurable with `workspaces-dir`). Shell
integration is optional, and `wsp setup` asks before appending one line to your
shell startup file. Each clone gets a few Git defaults, such as
`push.autoSetupRemote` and `rerere.enabled`; `wsp config ls` shows them and
`wsp config set git.<key> <value>` changes them. No `wsp`-specific remotes are
added.

**Setup commands need your approval.** A repository can declare post-clone
commands in its `.wsp.yaml`. `wsp` shows them and asks before running anything,
remembers your answer, and asks again if the commands change. This is the same
trust model as `direnv allow`.

**You are not locked in.** Workspaces are ordinary, standalone Git clones, and
they keep working without `wsp`.

**Uninstalling.** Remove the `wsp` binary (`brew uninstall wsp` for Homebrew
installs), delete the line `wsp setup` added to your shell startup file, and
optionally delete `~/.local/share/wsp`.

**Platforms.** Release builds cover macOS on Apple Silicon, Linux (x86_64 and
arm64), and Windows (x86_64 and arm64).

## Going further

- **Templates** save a set of repositories so you don't retype them:

  ```bash
  wsp template new refunds orders payments dashboard
  wsp new refund-flow-v2 -t refunds
  ```

  Share one with `wsp template export` and `wsp template import`.
- **Per-repo setup** lets a repository declare `setup_commands` in its
  `.wsp.yaml`. Re-run them with `wsp repo setup`, or let `wsp doctor --fix`
  finish repositories cloned without approval.
- **Shell integration** adds tab completion, moves you into a workspace after
  `wsp new`, and moves you out safely when you remove the one you are in.
  On macOS and Linux, `wsp setup` can configure it; the
  [usage guide](docs/usage.md) has the manual steps for each shell.

| Goal | Command |
|------|---------|
| Create a workspace | `wsp new <name> [repos...] [-t template]` |
| List workspaces | `wsp ls [-q]` (`-q` prints names only) |
| Enter a workspace | `wsp cd <workspace>` |
| See status across repos | `wsp st [workspace]` |
| Review changes across repos | `wsp diff [workspace] [-- args]` |
| View history across repos | `wsp log [workspace] [-- args]` |
| Fetch and rebase repos | `wsp sync [workspace]` |
| Run a command across repos | `wsp exec [workspace] -- <command>` |
| Add or remove repos | `wsp repo add/rm` |
| Rename a workspace | `wsp rename <old> <new>` |
| Remove workspaces | `wsp rm [workspace...]` |
| Recover a removed workspace | `wsp recover <workspace>` |

See the [full usage guide](docs/usage.md) or run `wsp help <command>` for
details.

## How it works

```text
~/.local/share/wsp/
└── mirrors/
    └── github.com/acme/
        ├── orders.git/        # bare mirrors; fetched once
        ├── payments.git/
        └── dashboard.git/

~/dev/workspaces/
├── refund-flow/
│   ├── .wsp.yaml              # workspace metadata
│   ├── AGENTS.md              # instructions for coding agents
│   ├── CLAUDE.md              # imports AGENTS.md
│   ├── .claude/skills/        # workspace skills for agents
│   ├── orders/                # normal local clones
│   ├── payments/
│   └── dashboard/
└── fix-retry/                 # same repos and layout, on its own branch
```

Registered repositories have a bare local mirror. Workspace clones reuse its
Git objects through local hardlinks where the filesystem allows it and copy them
otherwise, while each clone keeps a single `origin` pointing to the real
upstream.

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
just validate # smoke-test the release binary and generated CLI contract
just ci       # run the full CI pipeline, including validation
```

## License

[MIT](LICENSE)
