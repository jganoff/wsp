# Feature: Join Trailing Arguments for `wsp describe`

## Problem

When users set a workspace description that contains spaces, flag-like tokens, or UUIDs, they must quote the text:

```
wsp describe "claude --resume 120701c6-0630-4d09-ae3c-1503d4bfe743"
```

This fails without quotes because clap parses each token individually:

```
wsp describe -- claude --resume 120701c6-0630-4d09-ae3c-1503d4bfe743
# error: unexpected argument '120701c6-0630-4d09-ae3c-1503d4bfe743' found
```

The friction is highest when descriptions contain command-like text (the primary use case for workspace descriptions in practice), where the user is storing the command they ran to create the workspace or the task it represents.

## Proposed Behavior

Allow `--` to signal "everything after this is the description text, joined with spaces":

```
wsp describe -- claude --resume 120701c6-0630-4d09-ae3c-1503d4bfe743
# Sets description to: "claude --resume 120701c6-0630-4d09-ae3c-1503d4bfe743"
```

Quoting continues to work as before for backward compatibility:

```
wsp describe "claude --resume 120701c6-0630-4d09-ae3c-1503d4bfe743"
# Still works identically
```

## UX Assessment

### Recommendation: Yes, do this.

This is a good idea for three reasons:

1. **The use case is real and recurring.** Workspace descriptions frequently contain command fragments, branch names with slashes, or flag-like tokens. Forcing quotes on every such invocation is unnecessary friction that violates the "daily ops are muscle memory" tenet.

2. **The pattern already exists within wsp.** `wsp repo setup-commands add` and `wsp repo setup-commands rm` both use `.last(true)` + `.allow_hyphen_values(true)` + `.join(" ")` to accept multi-word input after `--`. This is not a novel invention; it is extending an established internal convention.

3. **The `--` semantics are compatible, not conflicting.** POSIX `--` means "stop parsing flags; treat remaining tokens as positional arguments." Collecting multiple positional tokens and joining them is a natural extension, not a violation. The user's mental model ("everything after `--` is my text") aligns with the actual behavior.

### Precedent in Other CLI Tools

**Within wsp itself:**
- `wsp exec my-ws -- make test` collects tokens after `--` as the command to run
- `wsp repo setup-commands add -- npm install` joins tokens after `--` into a single command string
- `wsp log -- --oneline --graph` passes tokens after `--` to git log
- `wsp diff -- --stat` passes tokens after `--` to git diff

**External tools:**
- `docker run` and `kubectl exec` use `--` to separate tool flags from the command passed to the container
- `npm run` uses `--` to forward arguments to the underlying script
- `cargo run -- <args>` passes trailing args to the built binary
- `git notes add -m "text"` uses a flag rather than positional for freeform text, but the underlying problem (freeform text on CLI) is the same

The pattern of collecting and joining trailing args after `--` is well-established. No user will be surprised by this behavior.

### Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| User expects standard `--` to keep args separate (not joined) | Low | `describe` has no other positional after `text`, so "separate but individual" has no useful meaning here. The only sensible interpretation is "this is all the description." |
| Whitespace normalization: multiple spaces between args collapse to one | Low | Acceptable. Shell already collapses whitespace before wsp sees it. Users needing exact whitespace should quote. Document this. |
| Backward compatibility: someone relying on the error | None | The current behavior is an error. Turning an error into success is always safe. |
| Confusion about when `--` is needed vs. when quoting is needed | Low | `--` is optional convenience. Quoting always works. Help text should show both forms. |

## Scope

### This feature applies to `wsp describe` only

`describe` is the only wsp command where:
- The payload is a single freeform string (not a structured value like a name or URL)
- The command does not already support `--` via `.last(true)`

Other commands that accept freeform text:
- `wsp new --description "text"` uses a named flag (`-d`/`--description`), so quoting is natural and expected
- `wsp template create --description "text"` same pattern
- `wsp rename` takes a workspace name (structured, no spaces)

If a future command accepts positional freeform text, it should follow the same pattern. But today, `describe` is the only one that benefits.

## Acceptance Criteria

1. `wsp describe -- any number of tokens here` sets the description to `"any number of tokens here"`.
2. `wsp describe -- --flag-like --tokens` sets the description to `"--flag-like --tokens"` (hyphen-prefixed tokens are not interpreted as flags).
3. `wsp describe "quoted text"` continues to work identically (backward compatible).
4. `wsp describe my-ws -- tokens after double dash` sets description for workspace `my-ws` to `"tokens after double dash"`.
5. `wsp describe -- ""` or `wsp describe --` with no trailing tokens clears the description (consistent with current empty-string behavior).
6. Help text (`wsp describe --help`) shows both forms: quoting and `--` syntax.
7. JSON output (`--json`) is unchanged; the mutation output structure is the same regardless of how the text was provided.

## Edge Cases

| Input | Expected behavior |
|-------|-------------------|
| `wsp describe --` (no text after) | Error: description text is required. Same as `wsp describe` with no args today. |
| `wsp describe -- ` (trailing whitespace only) | Clears description (trimmed to empty string). |
| `wsp describe my-ws --` (no text after) | Error: description text is required. |
| `wsp describe -- --json` | Sets description to `"--json"`. The `--json` flag must appear before `--` to be interpreted as a flag. |

## Implementation Notes (non-prescriptive)

The `setup-commands add` command in `repo_setup_commands.rs` already demonstrates the exact clap pattern needed. The text arg would use `.last(true)`, `.num_args(1..)`, and `.allow_hyphen_values(true)`, then join with `.join(" ")` in the handler. The existing two-positional disambiguation logic (`resolve_args`) needs to account for the text arg now being a `Vec<String>` instead of a single `String`.

## Alignment with Design Tenets

- **"Daily ops are muscle memory."** Removing the quoting requirement for the most common case (descriptions with spaces) reduces friction.
- **"No surprises."** The `--` behavior matches what users expect from other wsp commands and from the broader CLI ecosystem.
- **"Don't duplicate unix."** This does not add a flag or subcommand. It leverages existing POSIX convention.
- **"Progressive disclosure."** Quoting still works. `--` is available for users who discover it or see it in help text.
