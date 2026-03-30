---
name: wsp-report
description: Report a wsp issue on GitHub with full diagnostic context
user_invocable: true
---

# Report a wsp Issue

Gather diagnostic context and file a GitHub issue for the wsp tool.

## When to use

Use this skill when you encounter a bug, unexpected behavior, or error while using `wsp`. This skill collects everything needed for a useful bug report.

## Steps

### 1. Ask the user what went wrong

Before gathering diagnostics, ask the user to describe:
- What command they ran (or you ran) that failed
- What they expected to happen
- What actually happened
- The full error output (if not already visible in the conversation)

### 2. Gather diagnostic context

Run the following commands and capture the output:

```bash
wsp --version
uname -srm
echo $SHELL
wsp st --json 2>&1
wsp config ls --json 2>&1
```

**Do NOT run `wsp registry ls` by default.** The registry lists all repos the user has ever added, including private or internal ones. It is almost never needed to diagnose a bug. Only gather it if the bug is specifically about `wsp registry` commands.

### 3. Sanitize before sharing

**Redact ALL of the following before including any output in the issue:**

- **Filesystem paths**: Replace everything up to and including the username (e.g., `/Users/jganoff/dev/workspaces/...` → `~/dev/workspaces/...`). Do not leave any absolute paths containing a real username.
- **Branch names**: Replace with generic names like `<branch>` or `<workspace-branch>` unless the branch name is directly relevant to the bug (e.g., a parsing edge case). Branch names can reveal internal project names or codenames.
- **Config values**: From `wsp config ls --json`, include key names but redact values that could be personal: `workspaces-dir`, `branch-prefix`, git identity keys (`user.name`, `user.email`). Safe to include: booleans, numeric values, non-identifying strings like `sync-strategy`.
- **Workspace and repo names**: Replace with `<workspace>` and `<repo>` unless the name itself is the bug.
- **Tokens, passwords, credentials**: Remove entirely if they appear anywhere.

### 4. Show what will be shared — get explicit confirmation

**Before filing, show the user the complete sanitized diagnostics and issue body.** Say explicitly:

> "Here is everything that will be included in the GitHub issue. Please review before I file it."

Then list the sanitized content. Do not file until the user says to proceed.

### 5. Reproduce (if possible)

If the failing command can be safely re-run, execute it to capture fresh output. Include both stdout and stderr. If the command is destructive or has side effects, do NOT re-run it — use whatever output is already available.

### 6. Format the issue

```
Title: <type>: <concise description>
  - Types: bug, crash, unexpected-behavior

Body:
## Description
<1-3 sentences describing the problem>

## Steps to Reproduce
1. <exact commands>
2. ...

## Expected Behavior
<what should happen>

## Actual Behavior
<what actually happened, including error output>

## Environment
- wsp version: <output of wsp --version>
- OS: <output of uname -srm>
- Shell: <$SHELL>

## Workspace State
<sanitized output of wsp st --json, if relevant>

## Configuration
<sanitized output of wsp config ls --json, key names + safe values only>
```

Omit sections that are not relevant to the bug.

### 7. File the issue

Write the issue body to a temp file and use `--body-file` to avoid shell quoting issues:

```bash
gh issue create --repo jganoff/wsp --title "bug: <description>" --body-file /tmp/wsp-issue-body.md
```

### 8. Report back

Share the issue URL with the user after filing. Clean up any temp files.

## Notes

- If `gh` CLI is not available, format the issue as markdown and ask the user to paste it at https://github.com/jganoff/wsp/issues/new
- Include conversation context if relevant — what the agent was trying to do when the error occurred
- If the error involves a specific repo, include the repo's origin remote URL only if it's a public repo
- When in doubt about whether something is sensitive, leave it out
