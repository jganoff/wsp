---
name: testing-and-review
description: For agents contributing to this repo: write tests that can fail, and review your own diff before opening a PR
user_invocable: true
---

# Testing and reviewing wsp

Use this when writing tests, adding a guard, or finishing a change. The rules in
`AGENTS.md` say *what* is required; this says how, and what goes wrong.

## Prove every new test can fail

A test that passes whether or not the code works is worse than no test: it
occupies the space where a real one would go, and it reports confidence.

The procedure, every time:

1. Break the thing the test covers — delete the line, invert the condition,
   return the wrong value.
2. Run the test. **Watch it fail, and read the message.** A failure that does
   not name the problem will not help the next person either.
3. Restore. Re-run. Confirm green.

Do this before opening the PR, and say in the PR body which tests you verified
this way.

### Shapes that look like tests and are not

Each of these shipped in this repo and was caught by step 2:

- **Asserting only an exit status.** `wsp rm` refusing unsaved work asserted
  "did not exit 0" — satisfied by any unrelated failure. Assert the reason:
  `grep -qF "unsaved work"`.
- **Asserting an absence.** `template rm` asserted the template was gone, which
  a binary that does nothing at all satisfies. Pair it with a presence: remove
  one, assert the other survives.
- **Matching a string the command always prints.** A `sync` check matched
  `rebase`, which is the ACTION column and prints whether or not anything moved.
  It passed with the fixture removed. Match the RESULT: `fast-forwarded`.
- **A scoped assertion that matched the wrong scope.** A PowerShell test for
  `new` matched `rm`'s branch, because the search covered the whole generated
  wrapper. Scope to the branch under test (`case_body`).
- **Reading a value that is not populated yet.** `get_num_args()` returns `None`
  before `Command::build()`, so every assertion over it passed vacuously.
- **An environment that cannot reach the bug.** `grep -q` exits on its first
  match, which is what makes a writer see EPIPE — but this machine's `grep` is
  `ugrep`, which does *not* early-exit. The check passed locally whether the fix
  worked or not. When a test depends on a tool's behaviour, confirm the tool
  you have behaves that way: `/usr/bin/grep`, not whatever is first on `PATH`.

## Pick the cheapest test that can see the bug

| Where | Reaches | Cost |
|---|---|---|
| unit test in the module | pure logic, formatting, tables | free, runs always |
| `crates/wsp/tests/*.rs` | the real binary, one behaviour | fast, runs always |
| `tests/shell_cd.rs` | real shells loading the real wrapper | seconds |
| `scripts/smoke.sh` + `.ps1` | real binary, real filesystem, all 3 OSes | CI on every PR |

Work down the list, not up. Two rules that decide it for you:

- **If the bug lives in the interaction, a unit test cannot see it.** The panic
  hook matches a message std owns; a unit test on the matcher keeps passing
  while the binary reverts to dumping panics. So the wiring gets a behavioural
  test *and* the decision gets a unit test — they cover different failures.
- **If a unit test cannot state the dangerous direction, extract a seam.**
  `PanicHookInfo` cannot be constructed, so no test could assert that a *non*
  pipe failure still surfaces. Pulling the decision into a function over the
  message made `ENOSPC` testable.

## Structural guards

A guard is a test that reads the source or the CLI tree and fails when two
things that must agree stop agreeing. Write one when a mistake would otherwise
be silent, and there is a machine-checkable relationship.

Existing ones, as patterns to copy:

- `test_shellnav_matches_wrapper_cases` — every command's declared shell
  behaviour has a wrapper case, and vice versa.
- `test_every_command_is_smoked_or_listed_as_a_gap` — every command is smoked or
  named in `UNSMOKED`.
- `test_the_smoke_scripts_check_the_same_things` — the two smoke scripts assert
  the same labels.
- `the_cli_asks_where_the_user_is` — nothing under `cli/` reads the process cwd.

Three rules learned from writing them:

1. **Assert an equality, not a subset.** A register of known gaps must fail both
   when something loses coverage *and* when the register goes stale. Otherwise
   entries accumulate and nobody removes them.
2. **Fail closed.** If the guard cannot find what it is inspecting, that is a
   failure, not a pass. Add `assert!(found > 0)`.
3. **A source-scanning guard must not match itself.** Split the needle
   (`concat!("std::env::", "current_dir()")`) and skip comment lines, or prose
   about the rule reads as a breach of it. Anchoring on statement position
   instead is brittle: a check that grows an `if out=$(...)` wrapper is still a
   check, and a guard that cries wolf on every refactor teaches people to edit
   the register instead of reading it.

## The two smoke scripts are twins

`scripts/smoke.sh` and `scripts/smoke.ps1` must assert the same things, and the
label-parity guard enforces it. Beyond that:

- **Leave no state behind.** `doctor` runs later and exits non-zero on warnings,
  so a stray template or workspace surfaces as an unrelated failure several
  checks away.
- **Put the output in the failure message** when the check cannot say why it
  failed. An exit status is not self-explanatory; a `grep` for a known string is.
- **Run both before pushing** — `brew install powershell`, then `pwsh
  scripts/smoke.ps1 -Wsp ./target/release/wsp -Offline`. The PowerShell twin
  otherwise executes for the first time in CI.
- Interactive commands are testable: force a non-TTY stdin (`< /dev/null`, or an
  empty-string pipe in PowerShell) and the command takes its non-interactive
  path. That is what makes a check unable to hang.

## Reviewing your own diff

Read `git diff <base>..HEAD` end to end before opening. Hunt for:

- **Duplicated work** — the same value computed twice, one call that could serve
  both.
- **A comparison that should be a direct check.** Reconstructing which paths were
  deleted and comparing them against `$PWD` became `if !cwd.exists()`: shorter,
  and immune to the symlink mismatch the comparison had.
- **Claims that are not true on every platform.** "No process can exit with -1"
  holds on unix and not on Windows, where exit codes are 32-bit.
- **Archaeology in comments.** What the old version got wrong belongs in the
  commit message. See the module doc rule in `AGENTS.md`.
- **Tables and constants with no test.** A transposed signal number is invisible.
- **Anything deletable.** Prefer removing to adding.
- **Scope.** A wart you noticed on the line you were editing is a separate
  change, especially if it alters `--json` output.

Then re-run `just ci` and the smoke scripts, *then* open the PR.

## Where the answers live

- `AGENTS.md` — the rules, and the naming/wrapper contracts.
- `docs/design-tenets.md` — what to do when two goals conflict.
- `scripts/README.md` — what the smoke scripts cover and how to add a check.
- `RELEASING.md` — release notes and the dispatch flow.
