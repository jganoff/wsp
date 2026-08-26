---
name: wsp-release
description: Cut a wsp release (prep PR, dispatch, verify)
user_invocable: true
---

# Release wsp

**Read `RELEASING.md` first — it is the process. This file only covers what an
agent should do differently from a human following it.**

Argument: `<bump>` — `patch`, `minor`, `major`, or an explicit prerelease
version.

1. Stop if the tree is dirty. Run `just build` so later checks use current code.
2. `just changelog`, show the user the unreleased entries, and confirm the bump
   level matches them.
3. Run `just whatsnew-draft` to collect the notes each PR wrote for
   itself, then shape them into the `WHATSNEW.md` section per the rules in
   `RELEASING.md`. Prefer those notes over re-deriving prose from the
   changelog — the author had the context. **Show the draft and wait for
   approval** before continuing; the user may rewrite it.
4. Work through the flow in `RELEASING.md`. Do not merge the release PR
   yourself.
5. After it merges, run `just release-dispatch <version>` and report each job.
   Say plainly whether `publish-homebrew-formula` ran or skipped. The release
   body is filled in automatically; no manual edit.
Verify every `wsp <cmd>` in the notes against `wsp --help` before showing the
draft — do not rely on memory for command names.
