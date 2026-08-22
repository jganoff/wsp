---
name: wsp-release
description: Cut a wsp release (prep PR, dispatch, verify)
user_invocable: true
---

# Release wsp

Cut a new wsp release using `cargo-release` and `git cliff`.

## Arguments

- `<bump>` -- `patch`, `minor`, `major`, or an explicit version for a
  prerelease (e.g. `0.19.0-rc.1`)

## How releases land

Releases go through a reviewed PR, and **nobody pushes tags from a laptop**.

    just release-prep <bump>          # bump + changelog, commit on a branch
    <push branch, open PR, review, merge>
    just release-dispatch <version>   # CI creates the tag and releases

Two settings make that work, and both matter:

- `release.toml` sets `push = false` and `tag = false`, so `cargo-release` only
  prepares the commit. It must not tag: a squash merge rewrites the SHA, so a
  locally created tag would point at a commit that never lands on `main`.
- `dist-workspace.toml` sets `dispatch-releases = true`, so `release.yml`
  triggers on `workflow_dispatch` with a `tag` input instead of on tag pushes.
  CI creates the tag itself via `gh release create --target <commit>`.

Dispatching with no version (input `dry-run`, the default) plans the release
without publishing — useful for checking the pipeline.

You can dispatch from the GitHub Actions UI instead of a laptop — the Justfile
recipe is only a convenience wrapper.

Tags here are immutable: the tag ruleset blocks delete and update with no
bypass actors, so a wrong tag cannot be taken back. Two things guard against
that, and the second covers the UI as well:

- `just release-dispatch` refuses unless `origin/main`'s version matches the
  version asked for, which catches dispatching before the release PR has
  merged. It accepts `0.19.0` or `v0.19.0` and always tags with the `v`.
- cargo-dist refuses any tag that does not match a package version, and says
  which tag is correct. This happens in the `plan` job, and every other job
  declares `needs: plan`, so a typo fails *before* `gh release create` makes
  the tag. Nothing permanent happens.

        $ dist plan --tag=v9.9.9
        × This workspace doesn't have anything for dist to Release!
        help: --tag=v0.18.0 will Announce: wsp

Only the first check knows about the release PR, so dispatching a *valid but
not-yet-merged* version is the one mistake the UI will not catch for you.

Note a tagging *action* using the default `GITHUB_TOKEN` would not work here:
events raised by that token do not start new workflow runs, so a pushed tag
would never trigger `release.yml`. Dispatch avoids the problem entirely rather
than needing a PAT.

Prereleases (any version with a suffix, e.g. `0.19.0-rc.1`) are marked as
prereleases by `release.yml` and **skip the Homebrew publish**, so stable users
are unaffected. Write `WHATSNEW.md` notes for them exactly like any other
release: the `-rc.1` in the version heading already says it is a prerelease,
so do not add a preamble explaining that, why it was cut, or how to install it.

**A prerelease that tests well is rebuilt as the final version, not promoted.**
Reusing the artifacts is tempting and does not work: `wsp --version` comes from
`CARGO_PKG_VERSION`, baked in at compile time, so binaries built for
`0.19.0-rc.1` report that forever. Promoting them would ship a `0.19.0` release
whose binaries call themselves `0.19.0-rc.1`, and `wsp whatsnew` would look up
the wrong entry. cargo-dist has no promote command either. Cut the final
version from the same commit — same source, so the only thing that differs is
the version string, which is the thing that should differ.

Know what a prerelease does *not* cover: `publish-homebrew-formula` is the one
job skipped for prereleases, so the tap publish is untested until the real
release. It runs after `host`, so if it fails the GitHub release and binaries
already exist — re-run that job rather than re-cutting the release.

## Steps

### 1. Check tree is clean and build

```bash
git status
```

If dirty, stop and tell the user to commit or stash first.

Build a fresh release binary so all verification steps use the latest code:

```bash
just build
```

### 2. Preview the changelog

```bash
just changelog
```

Show the user the unreleased entries. Confirm the bump level is correct for the changes.

### 3. Generate release notes

Read the unreleased changelog entries from step 2, **and the last two or three
entries already in `WHATSNEW.md`** — those say what has already been announced,
which is the easiest thing to get wrong. Then write user-facing prose release
notes following the **Writing Release Notes** guidelines below.

Expect to drop most of the changelog. It lists every commit; `WHATSNEW.md`
lists what a user would notice.

Prepend the new version section to `WHATSNEW.md` (after the `# What's New` header).
Use the version number that will result from the bump (e.g. if current is 0.15.0 and
bump is `minor`, the new version is 0.16.0). Use today's date.

Show the draft to the user for review. Wait for approval before proceeding. The user
may edit the notes directly.

### 4. Prepare the release PR

Prepare the bump on a branch. `cargo-release` bumps `Cargo.toml`, regenerates
`CHANGELOG.md`, and commits — it does not tag and does not push.

```bash
git switch -c release/v<version>
just release-prep <bump>
git add WHATSNEW.md
git commit --amend --no-edit
git push -u origin release/v<version>
gh pr create --title "chore(release): v<version>"
```

Amending folds the release notes into the same commit, so the PR is one
self-contained change. Get it reviewed and merged before continuing.

### 5. Dispatch the release

Once the PR is merged:

```bash
just release-dispatch <version>
```

CI creates the tag against the merged commit and runs `release.yml`, which
builds binaries, creates the GitHub Release, and publishes to the Homebrew tap
(skipped automatically for prereleases).

Run `just release-dispatch` with no argument first if you want a dry run: it
plans the release without publishing anything.

### 6. Verify

- Check the GitHub Actions run: `gh run list --limit 5`
- Confirm the release appears: `gh release list --limit 3`
- Confirm the tag: `git tag --sort=-version:refname | head -3`

### 7. Update GitHub Release body

After confirming the GitHub release exists:

1. Read the version section from `WHATSNEW.md` for the just-released version.
2. Fetch the current release body: `gh release view v<version> --json body -q .body`
3. Prepend the prose notes to the existing body (which contains cargo-dist's
   install/download table).
4. Update the release: write the combined body to a temp file, then run
   `gh release edit v<version> --notes-file <tempfile>`.

This preserves cargo-dist's auto-generated install instructions while adding
user-facing context at the top.

## Writing Release Notes

Guidelines for generating the prose release notes in step 3.

### Audience

Developers who use wsp daily. Technical, impatient. Want to know what to
try and what got fixed.

### Tone

Direct and practical. Second person ("you"). Active voice. No hype words
(excited, thrilled, amazing, powerful, game-changing). No emoji. Understated
confidence, like a colleague mentioning a tool improvement. Match Fish shell
or Rust release notes.

- Bad: "We're thrilled to announce an amazing new feature!"
- Good: "`wsp st` now shows open pull requests for each repo."

### What to leave out

`WHATSNEW.md` is read by people who *use* wsp. Nothing else belongs in it.
Most bad release notes are bad because they were written from the maintainer's
seat.

- **Anything only a maintainer cares about.** CI, the release process,
  refactors, test coverage, lint rules, dependency bumps, internal API
  changes. If a user cannot observe it by running `wsp`, it is not a release
  note. Most entries in `git cliff` output fall here — the changelog is the
  complete record, `WHATSNEW.md` is the useful subset.
- **Proof of work.** "Builds and passes its full test suite on Windows",
  "now has 600 tests". Users care that it works, not how you know it works.
- **Implementation detail.** Say what happened to the reader, not what the
  code did.
  - Bad: "cloned from whatever the mirror last saw"
  - Good: "gave you whatever was last fetched, which could be weeks old"
- **Install instructions**, including for prereleases. The release page
  covers that, and it dates badly.
- **Why the release exists.** "Cut to validate the pipeline" is a maintainer
  concern.

**Read the previous entries before writing.** Re-announcing something wsp
already shipped makes the earlier release look empty and this one look
padded. wsp announced Windows support in 0.18.0, so a later note saying "wsp
now runs on Windows" is wrong even though nothing in it is false — the new
thing was PowerShell shell integration, and the rest were fixes. Describe only
what changed *this time*.

### Structure

Use this structure, omitting sections that don't apply:

1. **Breaking Changes** (only if present; always first)
   - What changed, why, and what the user needs to do
   - Include before/after command examples

2. **Highlights** (skip for patch releases or fewer than 3 features)
   - 1-3 sentence paragraph summarizing the release theme
   - Name the 2-3 most impactful changes
   - No bullet points; write it as a paragraph

3. **What's New** (skip for patch-only releases)
   - Group related changes by user-facing theme, not commit type
   - Each theme: `###` sub-heading, 1-2 sentences on what changed and why,
     fenced code block with a concrete command to try
   - Merge features and their related bug fixes into the same theme
   - Fewer than 3 features total: skip themed sub-headings, use a flat list
   - Order themes by impact: most noticeable first

4. **Fixes** (for bug fixes not already covered in themes)
   - Flat bulleted list, most impactful first
   - Each item: one sentence, start with the affected command or area

5. **Internal** (optional; only for removals, deprecations, API surface
   changes that power users or script authors might notice)
   - Skip refactors, test changes, CI changes with zero user impact

### Theming heuristics

1. Read all features and fixes
2. Identify clusters sharing a command, subsystem, or user workflow
3. Name clusters after the user-facing concept ("Branch tracking",
   "Setup commands"), not internals ("workspace.rs changes")
4. A theme needs 2+ related entries; orphans go in a flat list or fold
   into the nearest theme
5. Order by impact: the thing most users will notice first goes first

### Size calibration

- **Patch** (only fixes): "Bug-fix release." intro, then flat Fixes list
- **Small minor** (1-2 features): skip Highlights, flat What's New list
- **Standard minor** (3+ features): full structure with Highlights and
  themed sections
- **Major** (breaking changes): Breaking Changes first with migration guide

### Formatting rules

- ATX headings (`##`, `###`), not setext
- Fenced code blocks, no language tag. `wsp whatsnew` renders these
  as dimmed text, so inline comments must not rely on column alignment
  (put comments on their own line, or use short commands that don't
  need padding)
- Backtick-wrap command names, flags, files, config keys
- No HTML, no tables, no color codes
- Line-wrap prose at ~78 characters
- Use `wsp` not `WSP`; reference commands as typed: `wsp st` not
  "the status command"
- Do not start any sentence with "This release" or "In this version"
- No commit hashes, PR numbers, contributor acknowledgments, version
  numbers in headings, roadmap teasers, or apologies
- **Verify every command reference**: before finalizing, run
  `wsp --help` and confirm every `wsp <cmd>` mentioned in the notes
  is a real command. Do not reference commands that don't exist

### Reference example

See `WHATSNEW.md` in the repo root for examples of well-structured
release notes. The v0.15.0 entry demonstrates the full structure:
highlights paragraph, themed sections with command examples, flat
fixes list, and internal notes.

## Notes

- **`dist-workspace.toml` change?** If you modified dist config since the last release, run `dist generate` first to regenerate `.github/workflows/release.yml`. The workflow won't include new publish jobs until regenerated.
- **`HOMEBREW_TAP_TOKEN`** must be set as a repo secret for the Homebrew publish job to work.
- Dry runs modify `CHANGELOG.md` -- always `git checkout CHANGELOG.md` after a dry run before executing.
