# Telemetry

wsp collects a small amount of pseudonymous usage data by default, so the people building it know which commands and platforms are actually used. "Pseudonymous," not anonymous: a random ID links your reports together for as long as analytics stays on — see "The pseudonymous install ID" below for exactly what that does and doesn't mean. This page is the current, honest account of what that means; sections still marked pending will be finalized before any collection begins. If anything here surprises you, that's a bug in the feature, not in this page.

> **Status:** This page describes a proposal. Released versions of wsp do not
> currently send analytics or telemetry. wsp will update this notice before any
> collection begins. The underlying design is in
> [`docs/features/usage-telemetry.md`](features/usage-telemetry.md).

## The short version

- **On by default** — like Homebrew, Codex, and most CLI dev tools you already use.
- **Starts from your very first command.** The first-run notice (below) is a heads-up, not a gate — it doesn't delay or permit anything.
- **Turn it off in one command**: `wsp config set analytics.enabled false`.
- **Never your code, paths, names, arguments, or output.** The full boundary is below.

## What's collected

Running a command may send one small report containing:

| | |
|---|---|
| Command | e.g. `st`, `sync`, `repo/fetch` — never its arguments |
| Outcome | succeeded, or failed in an ordinary way — never an error message |
| If it failed, roughly why | a broad category like "network" or "permission" — never the actual error |
| Duration | a rough bucket like "100–249ms" — never an exact time |
| Version, OS, CPU architecture | e.g. `0.19.0`, macOS, arm64 |
| Install method | One of a small set of known channels (Homebrew, cargo, a GitHub release, etc.), or "unknown" — never a raw string from how you installed it |
| Running inside wsp's agent-sandbox mode | yes/no — wsp has a mode built for AI agents to run it against an isolated workspace with no other access; this just reports whether that mode is active, the same way the mode itself already works |
| Shell | a rough bucket (zsh/bash/fish/powershell/other) — never your exact shell path |
| Workspace size | a rough bucket like "3–5 repos", only for commands where that's relevant |
| How a workspace was created | template, derived from another workspace, from a file, empty, built from listed repos, or copied automatically — only on `wsp new` |
| `wsp doctor` result | which specific check(s) found something, not just a count — only on that command |

That's the entire list. Nothing else rides along.

## What's never collected

- Anything you typed: arguments, flags, workspace/repo/branch/org names
- Paths, URLs, your home directory, or your current directory
- File contents, git history, commit messages, diffs, or `AGENTS.md` content
- Anything wsp printed to your terminal, or any error text
- Your name, email, username, hostname, exact shell path, locale, or timezone
- Anything that identifies your machine or hardware

## Turning it off

```sh
wsp config set analytics.enabled false
```

or, if you'd rather use an environment variable (a script, a shared machine, your whole shell profile):

```sh
export DO_NOT_TRACK=1
# or, wsp-specific:
export WSP_NO_ANALYTICS=1
```

All three take effect immediately. `wsp config set analytics.enabled false` deletes the local install ID described below; the environment variables just pause sending without deleting anything, so removing them later picks back up where you left off.

There's no separate rule for CI. `analytics.enabled` (and the environment
variables above) apply the same way whether you're running wsp at a terminal
or in a build pipeline — there's no second setting to find and no detection
of "is this CI" happening behind the scenes.

## The first time you run wsp

The first eligible interactive command prints a one-line notice — something like:

> wsp sends small, pseudonymous usage reports by default — a random ID links them together, but never your name, code, or files. Run `wsp config set analytics.enabled false` to turn it off. Details: <link to this page>

It's shown once, only when wsp is running at an interactive terminal — never in `--json`, scripted, or CI use, the same way wsp already decides whether to show any other interactive prompt. That's just about where the notice can usefully be printed, though: analytics itself isn't gated on it, and is already on by default from the first command whether or not anyone was there to see the notice.

## The pseudonymous install ID

So reports can answer "how many installations use this command," not just "how many times was this command run," wsp generates a random ID the first time it sends a report and stores it locally next to its other settings, at `~/.local/share/wsp/analytics.yaml`. It's not your name, email, GitHub identity, or a hardware ID — just a random value. But be clear about what it does: it links every report your install sends into one history for as long as analytics stays on, which is what makes this pseudonymous rather than a one-off anonymous ping. Turning analytics off deletes it; turning it back on generates a fresh one, starting a new history.

## See for yourself

```sh
wsp config get analytics.enabled   # is it on
wsp doctor                          # includes an analytics status line: on/off, install ID present
```

## Where reports go

wsp sends reports to PostHog, an analytics provider, using a public write-only key that can only add events to wsp's own project — it can't read anything back. Every report also tells PostHog not to guess your location from your network address. A full account of retention, processing region, and data handling is being completed before any collection begins, and will replace this paragraph here and in [`docs/analytics.md`](analytics.md) once it is — that review, not this page, is what's still pending.
