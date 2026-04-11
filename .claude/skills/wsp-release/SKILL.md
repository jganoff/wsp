---
name: wsp-release
description: Cut a wsp release (dry-run, execute, verify)
user_invocable: true
---

# Release wsp

Cut a new wsp release using `cargo-release` and `git cliff`.

## Arguments

- `<bump>` -- `patch`, `minor`, or `major`

## Steps

### 1. Check tree is clean

```bash
git status
```

If dirty, stop and tell the user to commit or stash first.

### 2. Preview the changelog

```bash
just changelog
```

Show the user the unreleased entries. Confirm the bump level is correct for the changes.

### 3. Generate release notes

Read the unreleased changelog entries from step 2. Write user-facing prose release notes
following the **Writing Release Notes** guidelines below.

Prepend the new version section to `WHATSNEW.md` (after the `# What's New` header).
Use the version number that will result from the bump (e.g. if current is 0.15.0 and
bump is `minor`, the new version is 0.16.0). Use today's date.

Show the draft to the user for review. Wait for approval before proceeding. The user
may edit the notes directly.

### 4. Dry-run

```bash
just release <bump>
```

**Warning:** The dry-run executes the pre-release hook, which **modifies `CHANGELOG.md`**. After reviewing the dry-run output, restore it:

```bash
git checkout CHANGELOG.md
```

Show the user the dry-run output and ask for explicit confirmation before proceeding.

If the release is aborted after step 3, restore both files:

```bash
git checkout CHANGELOG.md WHATSNEW.md
```

### 5. Stage release notes and execute

Stage `WHATSNEW.md` so `cargo-release` includes it in the release commit (it only auto-stages files it modifies, like `Cargo.toml` and `CHANGELOG.md`):

```bash
git add WHATSNEW.md
just release-execute <bump>
```

This bumps `Cargo.toml`, regenerates `CHANGELOG.md`, commits (including the staged `WHATSNEW.md`), tags `v<version>`, and pushes. The tag push triggers `.github/workflows/release.yml` (cargo-dist) which builds binaries, creates a GitHub Release, and publishes to the Homebrew tap.

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
