# Feature: AGENTS.md Generation

Generate `AGENTS.md` (with a `CLAUDE.md` import) at the workspace root so AI agents have context about repos, branches, and available `wsp` commands.

## Motivation

When working inside a repo in a wsp workspace, AI agents (Claude Code, Cursor, etc.) have no visibility into workspace-level context: what other repos exist, what branch the workspace is on, or what `wsp` commands are available. Generating a standard agent context file at the workspace root solves this.

## File Layout

```
~/dev/workspaces/my-feature/
  AGENTS.md       <- generated, with marked sections
  CLAUDE.md       <- contains @AGENTS.md
  .wsp.yaml
  repo-a/
  repo-b/
```

- `AGENTS.md` is the primary file (cross-tool standard)
- `CLAUDE.md` is a regular file containing `@AGENTS.md` (Claude Code auto-discovers it when traversing up from child repos)

## File Format

```markdown
# Workspace: my-feature

<!-- Add your project-specific notes for AI agents here -->

<!-- wsp:begin -->
## Workspace Context

| Property | Value |
|----------|-------|
| Workspace | my-feature |
| Branch | jganoff/my-feature |

## Repositories

| Repo | Role | Ref | Directory |
|------|------|-----|-----------|
| github.com/acme/api-gateway | active | - | api-gateway |
| github.com/acme/proto | active | - | proto |

## Quick Reference

```bash
wsp st                  # status across all repos
wsp diff                # diff across all repos
wsp repo add <repo>     # add repo to workspace
wsp repo rm <repo>      # remove repo from workspace
wsp exec <name> -- cmd  # run command in each repo
```

<!-- wsp:end -->
```

## Marked Section Behavior

Content between `<!-- wsp:begin -->` and `<!-- wsp:end -->` is managed by wsp. Everything outside the markers is user-owned.

- **First creation:** Generate heading + human-editable placeholder + marked section
- **Updates:** Replace only content between markers, preserve everything else
- **Markers missing** (user deleted them): Append the marked block at the end
- **Markers malformed** (only begin, only end, or inverted): Treat as missing, append

## Claude Import Rules

- `CLAUDE.md` doesn't exist: create a regular file containing `@AGENTS.md` and a newline
- `CLAUDE.md` is a symlink to `AGENTS.md`: replace the link with the import file, preserving its target
- `CLAUDE.md` is a symlink pointing elsewhere: leave it alone (user-owned)
- `CLAUDE.md` is a regular file: leave it alone (user intentional)

The import is written atomically and works without symlink privileges on Windows.
`wsp doctor --fix` migrates generated links. Custom files containing the import
are valid; files without it need manual repair and are preserved. Removal checks
treat only the exact generated import as managed content, protecting user notes.

## Config

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub agent_md: Option<bool>,
```

- `None` or `Some(true)` = enabled (default on)
- `Some(false)` = disabled
- Disable via: `wsp config set agent-md false`

## Architecture

New top-level module `src/agentmd.rs`. Not a language integration because:

- Language integrations are conditional (`detect()` checks for `go.mod`, etc.). AGENTS.md is unconditional.
- Language integrations can be disabled via `config.language_integrations`. AGENTS.md gets its own config key.
- Must preserve human-written content outside markers (language integrations fully own their output).

Called from the same three CLI entry points (`new.rs`, `add.rs`, `remove.rs`) right after `lang::run_integrations()`.

### Public API

```rust
/// Generate or update AGENTS.md (and CLAUDE.md import) at the workspace root.
/// Failures produce warnings via eprintln, never abort the workspace operation.
pub fn update(ws_dir: &Path, metadata: &Metadata) -> Result<()>
```

### Internal Functions

- `build_marked_section(metadata: &Metadata) -> String` -- content between markers
- `build_initial_file(metadata: &Metadata) -> String` -- full scaffold for first creation
- `replace_marked_section(existing: &str, new_section: &str) -> String` -- parse + replace
- `ensure_claude_import(ws_dir: &Path) -> Result<()>` -- create the import or migrate a generated link

### Marker Parsing

```rust
const MARKER_BEGIN: &str = "<!-- wsp:begin -->";
const MARKER_END: &str = "<!-- wsp:end -->";

fn replace_marked_section(existing: &str, new_section: &str) -> String {
    let begin_idx = existing.find(MARKER_BEGIN);
    let end_idx = existing.find(MARKER_END);

    match (begin_idx, end_idx) {
        (Some(b), Some(e)) if b < e => {
            let end_of_marker = e + MARKER_END.len();
            let mut result = String::new();
            result.push_str(&existing[..b]);
            result.push_str(new_section);
            if end_of_marker < existing.len() {
                result.push_str(&existing[end_of_marker..]);
            }
            result
        }
        _ => {
            let mut result = existing.to_string();
            if !result.ends_with('\n') {
                result.push('\n');
            }
            result.push('\n');
            result.push_str(new_section);
            result
        }
    }
}
```

## Call Sites

All three entry points get the same pattern:

```rust
let agent_md_enabled = cfg.agent_md.unwrap_or(true);
if agent_md_enabled {
    if let Err(e) = crate::agentmd::update(&ws_dir, &meta) {
        eprintln!("warning: AGENTS.md generation failed: {}", e);
    }
}
```

- `src/cli/new.rs` -- after `lang::run_integrations()`
- `src/cli/add.rs` -- after `lang::run_integrations()`
- `src/cli/remove.rs` -- after `lang::run_integrations()`

## Files to Modify

**New:**
- `src/agentmd.rs`

**Modified:**
- `src/main.rs` -- add `mod agentmd;`
- `src/config.rs` -- add `agent_md: Option<bool>` field
- `src/cli/new.rs` -- call `agentmd::update()`
- `src/cli/add.rs` -- call `agentmd::update()`
- `src/cli/remove.rs` -- call `agentmd::update()`

## Testing

All tests in `#[cfg(test)] mod tests` inside `src/agentmd.rs`, table-driven.

**Marker parsing (pure string, no FS):**
- Markers present: content replaced
- Custom content before/after markers: preserved
- Only begin marker: treated as missing, appends
- Only end marker: treated as missing, appends
- No markers: appends
- Inverted markers: treated as missing, appends
- Empty file: appends

**Content generation:**
- Repo table with active, context, mixed, custom dirs
- Scaffold structure

**Filesystem integration (tempdir):**
- Creates new file with correct content
- Creates a regular CLAUDE.md import file
- Preserves human content outside markers
- Missing markers: appends
- Generated symlink, including a broken link to AGENTS.md: migrated without writing through it
- Unrelated symlink: left alone
- Regular CLAUDE.md file: left alone
- Empty repos: valid table

**Config integration:**
- `None` and `Some(true)` generate
- `Some(false)` skips

## Edge Cases

- **Windows:** generation uses regular files and does not require Developer Mode.
- **Concurrent operations:** Same race condition profile as `go.work` and metadata saving. Acceptable.
- **User writes markers in notes:** Extremely unlikely. Can make markers more unique later if needed (e.g., `<!-- wsp:managed-section:begin -->`).
