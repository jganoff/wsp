# Releasing

The whole process lives here. `/wsp-release` drives it but adds no rules of
its own.

## Flow

```bash
git switch -c release/v<version>
just release-prep <bump>          # bump + CHANGELOG, commit; no tag, no push
# write WHATSNEW.md notes, then fold them into that commit:
git add WHATSNEW.md && git commit --amend --no-edit
git push -u origin release/v<version>
gh pr create --title "chore(release): v<version>"
# ...review, merge...
just release-dispatch <version>   # CI creates the tag and releases
```

`<bump>` is `patch`, `minor`, `major`, or an explicit prerelease version
(`0.19.0-rc.1`). Notes must be written *after* `release-prep`: cargo-release
requires a clean tree.

## Why it works this way

Nobody pushes tags from a laptop, and the version bump gets reviewed like any
other change.

- `release.toml` sets `push=false tag=false`, so cargo-release only prepares a
  commit. It must not tag: a squash merge rewrites the SHA, so a local tag
  would point at a commit that never lands on `main`.
- `dist-workspace.toml` sets `dispatch-releases=true`, so `release.yml` runs on
  `workflow_dispatch` and CI creates the tag via `gh release create --target`.
  You can dispatch from the Actions UI; the Justfile recipe is a convenience.

Do not replace this with a tagging action. Tags pushed by the default
`GITHUB_TOKEN` do not trigger workflows, so `release.yml` would never run.

Tags are immutable (the ruleset blocks delete and update, no bypass). Two
guards: `release-dispatch` refuses unless `origin/main`'s version matches, and
cargo-dist rejects any tag not matching a package version — in `plan`, which
every job needs, so a typo fails before the tag exists. Only the first knows
whether the PR merged, so a valid-but-unmerged version is the one mistake the
UI won't catch.

## Prereleases

Any suffixed version is flagged prerelease and **skips the Homebrew publish** —
the only skipped job, so the tap publish stays untested until a real release.
It runs after `host`, so if it fails the release and binaries already exist;
re-run that job rather than re-cutting.

**Rebuild the final version, don't promote the rc.** `wsp --version` comes from
`CARGO_PKG_VERSION` at compile time, so rc artifacts report `-rc.1` forever and
`wsp whatsnew` would look up the wrong entry. cargo-dist has no promote command.

## Writing release notes

`WHATSNEW.md` is compiled into the binary and shown by `wsp whatsnew`. Prepend
a section under `# What's New` using the post-bump version and today's date.

Written for people who **use** wsp: technical, impatient, want to know what to
try and what got fixed. Second person, active voice, no hype, no emoji.

**Read the last few entries first.** Re-announcing shipped work makes the old
release look empty and this one padded. 0.18.0 already announced Windows, so
"wsp now runs on Windows" was wrong despite being true — the new thing was
PowerShell shell integration, the rest were fixes.

Leave out:

- Maintainer-only changes: CI, release process, refactors, tests, deps,
  internal APIs. If a user can't observe it by running `wsp`, it isn't a note.
  Expect to drop most of the changelog — that's the full record, this is the
  useful subset.
- Proof of work: "passes its full test suite", "now has 600 tests".
- Implementation detail. Not "cloned from whatever the mirror last saw" but
  "gave you whatever was last fetched, which could be weeks old".
- Install instructions, including for prereleases.
- Why the release exists ("cut to validate the pipeline").

Structure, omitting what doesn't apply: **Breaking Changes** (first; what to
do, before/after) → **Highlights** (only for 3+ features; one paragraph) →
**What's New** (`###` per user-facing theme, 1-2 sentences + a command to try;
themes need 2+ entries; under 3 features use a flat list) → **Fixes** (flat,
most impactful first, each starting with the command) → **Internal** (only
removals or deprecations a user could hit).

Format: ATX headings; fenced blocks with no language tag (`wsp whatsnew` dims
them, so don't rely on column alignment); backtick commands, flags, files; no
HTML, tables, or color; wrap ~78 chars; `wsp st`, not "the status command". No
"This release…", commit hashes, PR numbers, or version numbers in headings.
**Verify every command exists** with `wsp --help`. The v0.15.0 entry is a good
full-structure example.

## After the release

Prepend the `WHATSNEW.md` section to the GitHub release body, which otherwise
holds only cargo-dist's install table:

```bash
gh release view v<version> --json body -q .body   # combine with the notes
gh release edit v<version> --notes-file <file>
```

Verify with `gh run list --limit 5` and `gh release list --limit 3`.

## Setup and gotchas

```bash
cargo install cargo-release git-cliff cargo-dist
```

- Changed `dist-workspace.toml`? Run `dist generate` before releasing.
- `HOMEBREW_TAP_TOKEN` must be set for the Homebrew publish job.
- Binaries are built for five targets: macOS x86_64/aarch64, Linux
  x86_64/aarch64, Windows x86_64/aarch64 (see `dist-workspace.toml`).

| File | Purpose |
|---|---|
| `release.toml` | cargo-release: tag format, hooks, `push`/`tag`/`publish` off |
| `cliff.toml` | git-cliff: commit grouping, changelog template |
| `dist-workspace.toml` | cargo-dist: targets, installers, `dispatch-releases` |
| `.github/workflows/release.yml` | generated by `dist generate` — never hand-edit |
