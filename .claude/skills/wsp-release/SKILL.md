---
name: wsp-release
description: Cut a wsp release (prep PR, dispatch, verify)
user_invocable: true
---

# Release wsp

`<bump>` is `patch`, `minor`, `major`, or an explicit prerelease version
(`0.19.0-rc.1`).

## Flow

```
just release-prep <bump>          # bump + CHANGELOG, commit on a branch
<push, open PR, review, merge>
just release-dispatch <version>   # CI creates the tag and releases
```

Nobody pushes tags from a laptop. `release.toml` sets `push=false tag=false`
(a squash merge rewrites the SHA, so a local tag would point at a commit that
never lands); `dist-workspace.toml` sets `dispatch-releases=true`, so
`release.yml` runs on `workflow_dispatch` and CI creates the tag via
`gh release create --target`. Dispatching is also possible from the Actions UI.

Do not replace this with a tagging action: tags pushed by the default
`GITHUB_TOKEN` do not trigger workflows, so `release.yml` would never run.

Tags are immutable (ruleset blocks delete/update, no bypass). Two guards:
`release-dispatch` refuses unless `origin/main`'s version matches, and
cargo-dist rejects any tag not matching a package version — in `plan`, which
every job needs, so a typo fails before the tag exists. Only the first guard
knows whether the PR merged, so that is the one mistake the UI won't catch.

## Prereleases

Any suffixed version is flagged prerelease and **skips the Homebrew publish**.
That is the only skipped job, so the tap publish stays untested until the real
release; it runs after `host`, so if it fails, re-run that job rather than
re-cutting.

**Rebuild the final version, don't promote the rc.** `wsp --version` comes from
`CARGO_PKG_VERSION` at compile time, so rc artifacts report `-rc.1` forever and
`wsp whatsnew` would look up the wrong entry. cargo-dist has no promote command.

## Steps

1. **Clean tree + fresh build.** `git status`; stop if dirty. `just build`.
2. **Changelog.** `just changelog`. Show the user; confirm the bump level.
3. **Notes.** Write them (see below), prepend to `WHATSNEW.md` under
   `# What's New`, with the post-bump version and today's date. Show the draft
   and wait for approval.
4. **Release PR.**
   ```bash
   git switch -c release/v<version>
   just release-prep <bump>
   git add WHATSNEW.md && git commit --amend --no-edit
   git push -u origin release/v<version>
   gh pr create --title "chore(release): v<version>"
   ```
   Amending keeps the PR one self-contained change. Merge before continuing.
5. **Dispatch.** `just release-dispatch <version>`. With no argument it dry-runs
   (plans, publishes nothing).
6. **Verify.** `gh run list --limit 5`, `gh release list --limit 3`.
7. **Release body.** Prepend the `WHATSNEW.md` section for this version to the
   existing body (which holds cargo-dist's install table):
   `gh release view v<version> --json body -q .body`, combine, then
   `gh release edit v<version> --notes-file <file>`.

## Writing release notes

For people who **use** wsp. Technical, impatient, want to know what to try and
what got fixed. Second person, active voice, no hype, no emoji — Fish or Rust
release notes, not a launch announcement.

**Read the last few `WHATSNEW.md` entries first.** Re-announcing shipped work
makes the old release look empty and this one padded. 0.18.0 already announced
Windows, so "wsp now runs on Windows" was wrong despite being true — the new
thing was PowerShell shell integration, the rest were fixes.

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

**Verify every command exists** with `wsp --help` before finalizing.

`WHATSNEW.md`'s v0.15.0 entry is a good full-structure example.

## Notes

- Changed `dist-workspace.toml`? Run `dist generate` before releasing.
- `HOMEBREW_TAP_TOKEN` must be set for the Homebrew publish job.
