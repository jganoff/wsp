---
name: wsp-release
description: Cut a wsp release (dry-run, execute, verify)
user_invocable: true
---

# Release wsp

Cut a new wsp release using `cargo-release` and `git cliff`.

## Arguments

- `<bump>` — `patch`, `minor`, or `major`

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

### 3. Dry-run

```bash
just release <bump>
```

**Warning:** The dry-run executes the pre-release hook, which **modifies `CHANGELOG.md`**. After reviewing the dry-run output, restore it:

```bash
git checkout CHANGELOG.md
```

Show the user the dry-run output and ask for explicit confirmation before proceeding.

### 4. Execute

```bash
just release-execute <bump>
```

This bumps `Cargo.toml`, regenerates `CHANGELOG.md`, commits, tags `v<version>`, and pushes. The tag push triggers `.github/workflows/release.yml` (cargo-dist) which builds binaries, creates a GitHub Release, and publishes to the Homebrew tap.

### 5. Verify

- Check the GitHub Actions run: `gh run list --limit 5`
- Confirm the release appears: `gh release list --limit 3`
- Confirm the tag: `git tag --sort=-version:refname | head -3`

## Notes

- **`dist-workspace.toml` change?** If you modified dist config since the last release, run `dist generate` first to regenerate `.github/workflows/release.yml`. The workflow won't include new publish jobs until regenerated.
- **`HOMEBREW_TAP_TOKEN`** must be set as a repo secret for the Homebrew publish job to work.
- Dry runs modify `CHANGELOG.md` — always `git checkout CHANGELOG.md` after a dry run before executing.
