---
name: wsp-release-notes
description: Draft wsp release notes from merged PR whatsnew blocks, removing stale notes and shaping the result for users. Use when preparing WHATSNEW.md for a wsp release, separately from dispatching the release.
user_invocable: true
---

# Draft wsp release notes

Read `RELEASING.md`, especially "Writing release notes", before editing. It is
the source of truth for content, structure, and formatting.

Run `just whatsnew-draft`, optionally passing the specific previous tag supplied
by the user. Treat the result as raw material rather than text to paste.

Before drafting:

- Compare every note with the commits that followed it. Delete work that was
  reverted and replace descriptions superseded by later behavior.
- Combine multiple symptoms of one cause into one user-facing note and order
  notes by impact.
- Check recent `WHATSNEW.md` entries so the draft does not re-announce work that
  already shipped.
- Verify every mentioned command and flag against `wsp --help` and the relevant
  subcommand help.

Write the new section in `WHATSNEW.md` using the post-bump version and current
date. Follow all format rules in `RELEASING.md`, including the ban on em dashes.
Do not dispatch a release, push, tag, or open a pull request unless the user
separately asks for that action.
