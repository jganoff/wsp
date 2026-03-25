# Feature Roadmap

The roadmap has moved to GitHub Issues.

Filter by priority label:

- [P1 -- Adoption](https://github.com/jganoff/wsp/issues?q=is%3Aopen+label%3AP1)
- [P2 -- Ecosystem](https://github.com/jganoff/wsp/issues?q=is%3Aopen+label%3AP2)
- [P3 -- Polish](https://github.com/jganoff/wsp/issues?q=is%3Aopen+label%3AP3)
- [P4 -- Ideas / Needs Design](https://github.com/jganoff/wsp/issues?q=is%3Aopen+label%3AP4)

## Design Principles

See [`docs/design-tenets.md`](design-tenets.md) for the authoritative list. Summary:

- Every command is **workspace-aware** (workspace vs. upstream branches)
- Daily ops are **top-level short commands** (`sync`, `log`)
- **Always support `--json`** for scripting and AI agents
- **Parallel by default** for reads, serial for writes
- **Prevent data loss by default** -- destructive operations use deferred cleanup; permanent deletion is opt-in
- **Surface hidden state** -- wrong-branch, detached HEAD, and other mismatches are surfaced, not hidden
- **Workspace as context** -- the workspace metadata (`.wsp.yaml`, AGENTS.md, generated workspace files) is a coordination primitive consumed by AI agents, IDEs, and build tools
- No new external dependencies unless justified (`gh` for import/PR awareness is the exception)
