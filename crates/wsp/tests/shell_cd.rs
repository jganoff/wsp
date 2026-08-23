//! Behavioral tests for the shell wrapper's directory-changing logic.
//!
//! These spawn real shells, source the generated wrapper, run a command, and
//! assert on the resulting `$PWD`. They deliberately assert on *behavior*
//! rather than on the generated shell text, because every prior guarantee
//! about the wrapper was a string assertion — which is precisely how the
//! `wsp create` alias came to be missing from the cd path while the unit tests
//! stayed green.
//!
//! Two properties matter and are easy to get wrong:
//!
//! 1. **No-rc invocation.** Shells are launched with rc files disabled
//!    (`zsh -f`, `bash --norc`, ...). A user's `~/.zshenv` that exports
//!    `XDG_DATA_HOME` will otherwise clobber the test environment — and since
//!    that variable selects the config that selects `workspaces_dir`, the test
//!    would create and *delete* workspaces in the developer's real
//!    `~/dev/workspaces`. This is not hypothetical; it happened while writing
//!    these tests.
//!
//! 2. **Hermetic scenarios.** Each case gets a fresh data dir and workspaces
//!    dir, and performs its own setup by calling the binary directly (not
//!    through a shell). No case depends on another's leftovers.

use std::path::{Path, PathBuf};
use std::process::Command;

const WSP: &str = env!("CARGO_BIN_EXE_wsp");

// ---------------------------------------------------------------------------
// Shell definitions
// ---------------------------------------------------------------------------

/// How to wrap a path in single quotes for this dialect.
///
/// `SHELLS` is `cfg`-split, so exactly one variant is unused on any given
/// platform — `Posix` on Windows, `PowerShell` everywhere else.
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum Quote {
    /// Close, escape, reopen: `'` → `'\''`
    ///
    /// Covers fish too. The generator escapes fish differently (`'` → `\'`,
    /// see the security notes in AGENTS.md) because it quotes *inside* a
    /// single-quoted string, but this form closes the quote first and fish
    /// concatenates adjacent tokens exactly as POSIX shells do.
    Posix,
    /// Double the quote: `'` → `''`
    PowerShell,
}

/// Everything that differs between dialects lives here, so adding a shell means
/// adding a row rather than threading `cfg!` checks through the harness.
struct Shell {
    /// Executable name.
    bin: &'static str,
    /// Flags that disable rc/profile loading, plus the "run this string" flag.
    /// The script is appended as the final argument.
    args: &'static [&'static str],
    /// Snippet that loads the wrapper. `WSPBIN` is replaced with the quoted
    /// path to the binary under test.
    load: &'static str,
    /// Prints the current directory followed by a newline.
    print_pwd: &'static str,
    /// Where to send the command's own output.
    null: &'static str,
    /// Prints the exit status of the previous command.
    print_status: &'static str,
    quote: Quote,
}

#[cfg(unix)]
const SHELLS: &[Shell] = &[
    Shell {
        bin: "bash",
        args: &["--noprofile", "--norc", "-c"],
        load: r#"eval "$(WSPBIN completion bash)" 2>/dev/null"#,
        print_pwd: r#"printf '%s\n' "$PWD""#,
        null: "/dev/null",
        print_status: r#"printf '%s\n' "$?""#,
        quote: Quote::Posix,
    },
    Shell {
        bin: "zsh",
        // -f == NO_RCS: skip .zshenv/.zshrc entirely.
        args: &["-f", "-c"],
        load: r#"eval "$(WSPBIN completion zsh)" 2>/dev/null"#,
        print_pwd: r#"printf '%s\n' "$PWD""#,
        null: "/dev/null",
        print_status: r#"printf '%s\n' "$?""#,
        quote: Quote::Posix,
    },
    Shell {
        bin: "fish",
        args: &["--no-config", "-c"],
        load: r#"WSPBIN completion fish 2>/dev/null | source"#,
        print_pwd: r#"printf '%s\n' "$PWD""#,
        null: "/dev/null",
        print_status: r#"echo $status"#,
        quote: Quote::Posix,
    },
];

#[cfg(windows)]
const SHELLS: &[Shell] = &[Shell {
    bin: "pwsh",
    args: &["-NoProfile", "-NonInteractive", "-Command"],
    load: r#"Invoke-Expression ((& WSPBIN completion powershell) -join "`n")"#,
    print_pwd: r#"Write-Output $PWD.Path"#,
    null: "$null",
    // $LASTEXITCODE is $null until a native command has run in the session.
    print_status: r#"if ($null -eq $LASTEXITCODE) { Write-Output 0 } else { Write-Output $LASTEXITCODE }"#,
    quote: Quote::PowerShell,
}];

// ---------------------------------------------------------------------------
// Scenario table
// ---------------------------------------------------------------------------

/// Where the shell starts before running the command under test.
enum Start {
    /// The workspaces root itself.
    Root,
    /// Inside workspace `<name>`.
    Ws(&'static str),
    /// Inside `<name>/<subdir>`, which the harness creates. Covers standing
    /// deeper than the workspace root — the case the wrapper's old
    /// `"$wsp_dir"/*` glob existed to catch.
    WsSub(&'static str, &'static str),
}

/// Where the shell is expected to end up.
enum Expect {
    /// The workspaces root.
    Root,
    /// Inside workspace `<name>`.
    Ws(&'static str),
    /// Wherever it started — the command must not move the shell.
    Unchanged,
}

struct Scenario {
    name: &'static str,
    /// Run with the binary directly (no shell) to build the fixture.
    setup: &'static [&'static [&'static str]],
    start: Start,
    /// The `wsp ...` invocation under test, as it would be typed.
    cmd: &'static [&'static str],
    expect: Expect,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "new cds into the new workspace",
        setup: &[],
        start: Start::Root,
        cmd: &["new", "w", "--empty"],
        expect: Expect::Ws("w"),
    },
    Scenario {
        // The regression that motivated this file: `create` is a visible alias
        // for `new`, and the wrapper used to dispatch on the literal name only.
        name: "create alias cds like new",
        setup: &[],
        start: Start::Root,
        cmd: &["create", "w", "--empty"],
        expect: Expect::Ws("w"),
    },
    Scenario {
        // Was broken on posix/fish (cd to `<root>/--empty`) and correct on
        // pwsh. Fixed for all dialects by taking the destination from the
        // binary instead of argv.
        name: "new with a leading flag cds",
        setup: &[],
        start: Start::Root,
        cmd: &["new", "--empty", "w"],
        expect: Expect::Ws("w"),
    },
    Scenario {
        // A value-taking flag's *value* must never be mistaken for the
        // workspace name. pwsh's old "first non-flag arg" scan picked `notaws`
        // here and cd'd to `<root>/notaws`.
        //
        // `-d` is used rather than `-w`/`-t`/`-f` because those need a source
        // with repos in it, which cannot be built offline (#91). The argv
        // hazard is identical either way: a flag that consumes the token after
        // it.
        name: "new does not cd to a flag value",
        setup: &[],
        start: Start::Root,
        cmd: &["new", "-d", "notaws", "w", "--empty"],
        expect: Expect::Ws("w"),
    },
    Scenario {
        name: "cd enters the workspace",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Root,
        cmd: &["cd", "w"],
        expect: Expect::Ws("w"),
    },
    Scenario {
        // Guards the "does this cd for every command?" worry: read-only
        // commands must leave the shell where it is.
        name: "st does not move the shell",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Ws("w"),
        cmd: &["st"],
        expect: Expect::Unchanged,
    },
    Scenario {
        // Must escape before the directory is removed underneath us. On Windows
        // a live process's cwd cannot be deleted at all, so this is load-bearing
        // rather than cosmetic.
        name: "rm escapes the workspace being removed",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Ws("w"),
        cmd: &["rm", "w", "--force"],
        expect: Expect::Root,
    },
    Scenario {
        // Bare form: no workspace name, resolved from the directory the user was
        // standing in. The wrapper vacates before invoking, so this only works
        // if the original cwd is forwarded — it regressed exactly here.
        name: "bare rm resolves the workspace from the cwd",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Ws("w"),
        cmd: &["rm", "--force", "--yes"],
        expect: Expect::Root,
    },
    Scenario {
        // Prefix hazard: removing `w` while standing in `w-extra` must leave the
        // shell alone. Previously guarded by asserting the wrapper's comparison
        // required a separator; now that there is no comparison at all, assert
        // the behavior instead.
        name: "rm of a name-prefix workspace does not move the shell",
        setup: &[&["new", "w", "--empty"], &["new", "w-extra", "--empty"]],
        start: Start::Ws("w-extra"),
        cmd: &["rm", "w", "--force"],
        expect: Expect::Unchanged,
    },
    Scenario {
        name: "remove alias escapes like rm",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Ws("w"),
        cmd: &["remove", "w", "--force"],
        expect: Expect::Root,
    },
    Scenario {
        // Standing deeper than the workspace root. The wrapper used to need a
        // `"$wsp_dir"/*` glob for this; vacate-and-return handles it with no
        // special case, so assert it still does.
        name: "rm from a nested directory escapes",
        setup: &[&["new", "w", "--empty"]],
        start: Start::WsSub("w", "nested/deeper"),
        cmd: &["rm", "w", "--force"],
        expect: Expect::Root,
    },
    Scenario {
        // The failure branch of vacate-and-return: the command errors, so the
        // starting directory survives and the shell must come back to it rather
        // than being left at the root. Removing a nonexistent workspace is the
        // only way to fail `rm` hermetically — a blocked removal needs repos
        // with unmerged branches, which cannot be built offline (#91).
        name: "failed rm returns to where it started",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Ws("w"),
        cmd: &["rm", "nope", "--force"],
        expect: Expect::Unchanged,
    },
    Scenario {
        name: "recover cds into the restored workspace",
        setup: &[&["new", "w", "--empty"], &["rm", "w", "--force"]],
        start: Start::Root,
        cmd: &["recover", "w"],
        expect: Expect::Ws("w"),
    },
    Scenario {
        // `recover ls` is a subcommand, not a workspace name — it must not be
        // treated as one and cd'd into.
        name: "recover ls does not move the shell",
        setup: &[&["new", "w", "--empty"], &["rm", "w", "--force"]],
        start: Start::Root,
        cmd: &["recover", "ls"],
        expect: Expect::Unchanged,
    },
];

// Deliberately absent: `wsp new -b <branch>` derives the workspace name from
// the branch's last segment, so the wrapper has no positional to read and cds
// nowhere. It cannot be covered hermetically — `-b` requires the branch to
// exist in a real repo, and local-path repos are not supported yet (#91), so
// there is no way to build the fixture offline. Revisit alongside #69.

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// An isolated data dir + workspaces dir. Both live under one tempdir so a
/// misconfigured lookup cannot escape into the developer's real state.
struct Env {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    ws_root: PathBuf,
}

/// Canonicalize without the Windows `\\?\` verbatim prefix.
///
/// This matters for more than tidiness. `std::env::temp_dir()` on Windows can
/// hand back an 8.3 short path (`C:\Users\RUNNER~1\...`), and that value ends up
/// baked into the wrapper as `$wspRoot`. PowerShell reports `$PWD.Path` in long
/// form, so the wrapper's `$PWD.Path -eq $wspDir` guard compares a short path
/// against a long one, never matches, and `wsp rm` runs with the shell still
/// inside the workspace it is removing. Normalizing here keeps the config, the
/// wrapper, and the shell talking about the same spelling.
///
/// The `\\?\` prefix has to go: PowerShell's `$PWD.Path` never uses it, so
/// leaving it on would reintroduce the same mismatch from the other direction.
fn real_path(p: &Path) -> PathBuf {
    let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = c.to_string_lossy().into_owned();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}

fn make_env() -> Env {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = real_path(tmp.path());
    let data = root.join("data");
    let ws_root = root.join("ws");
    std::fs::create_dir_all(data.join("wsp")).expect("create data dir");
    std::fs::create_dir_all(&ws_root).expect("create ws root");
    // Point workspaces_dir at the tempdir. Without this the binary would fall
    // back to $HOME/dev/workspaces.
    // Single-quoted so a Windows temp path (`C:\Users\...`) stays a literal
    // scalar rather than relying on YAML plain-scalar rules around `:` and `\`.
    std::fs::write(
        data.join("wsp").join("config.yaml"),
        format!(
            "workspaces_dir: '{}'\n",
            ws_root.display().to_string().replace('\'', "''")
        ),
    )
    .expect("write config");
    Env {
        _tmp: tmp,
        data,
        ws_root,
    }
}

/// Apply the isolating environment. `HOME` is redirected too, so even a lookup
/// that ignores `XDG_DATA_HOME` lands inside the tempdir rather than in real
/// user state.
fn apply_env(cmd: &mut Command, env: &Env) {
    cmd.env("XDG_DATA_HOME", &env.data);
    cmd.env("HOME", env._tmp.path());
    cmd.env("USERPROFILE", env._tmp.path());
    // Advice hints go to stderr (discarded) and their cooldown state is written
    // under the redirected data dir, so no suppression is needed.
}

/// Run the binary directly, no shell. Used for fixture setup.
fn run_setup(env: &Env, args: &[&str]) {
    let mut cmd = Command::new(WSP);
    apply_env(&mut cmd, env);
    let out = cmd
        .args(args)
        .current_dir(&env.ws_root)
        .output()
        .expect("spawn wsp for setup");
    assert!(
        out.status.success(),
        "setup `wsp {}` failed\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Canonicalize for comparison. Needed because macOS `TMPDIR` lives under
/// `/var`, a symlink to `/private/var`, so a shell's `$PWD` and Rust's view of
/// the same directory are spelled differently. Falls back to the input when the
/// path no longer exists (e.g. a directory that was just removed).
fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Is this shell installed? Probed once per shell so a missing one does not
/// waste a fixture setup (which spawns the binary several times).
fn is_installed(shell: &Shell) -> bool {
    match Command::new(shell.bin)
        .args(shell.args)
        .arg(shell.print_pwd)
        .output()
    {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => panic!("spawning {} failed: {e}", shell.bin),
    }
}

/// Returns the shell's final `$PWD` plus whatever the command wrote to stderr,
/// so an assertion failure can explain *why* the shell ended up where it did.
/// Without this a Windows-only failure is just two paths and no cause.
fn run_in_shell(shell: &Shell, env: &Env, start: &Path, cmd: &[&str]) -> (PathBuf, String) {
    let load = shell.load.replace("WSPBIN", &quote(shell.quote, WSP));

    // Scenario args are static identifiers and flags, so joining them without
    // per-arg quoting is safe here. Only stdout is discarded, so the trailing
    // $PWD line is the sole stdout content; stderr is kept for diagnostics.
    let script = format!(
        "{load}\ncd {}\nwsp {} >{null}\n{}",
        quote(shell.quote, &start.to_string_lossy()),
        cmd.join(" "),
        shell.print_pwd,
        null = shell.null,
    );

    let mut c = Command::new(shell.bin);
    apply_env(&mut c, env);
    let out = c
        .args(shell.args)
        .arg(&script)
        .output()
        .unwrap_or_else(|e| panic!("spawning {} failed: {e}", shell.bin));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or_else(|| {
            panic!(
                "{} produced no output\nscript:\n{script}\nstderr: {}",
                shell.bin,
                String::from_utf8_lossy(&out.stderr)
            )
        });
    (
        PathBuf::from(last.trim()),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

fn quote(style: Quote, s: &str) -> String {
    match style {
        Quote::Posix => format!("'{}'", s.replace('\'', r"'\''")),
        Quote::PowerShell => format!("'{}'", s.replace('\'', "''")),
    }
}

#[test]
fn shell_wrapper_cd_behavior_matches_across_shells() {
    let mut ran = 0usize;
    let mut skipped: Vec<&str> = Vec::new();

    for shell in SHELLS {
        if !is_installed(shell) {
            skipped.push(shell.bin);
            continue;
        }

        for sc in SCENARIOS {
            let env = make_env();
            for args in sc.setup {
                run_setup(&env, args);
            }

            let start = match sc.start {
                Start::Root => env.ws_root.clone(),
                Start::Ws(n) => env.ws_root.join(n),
                Start::WsSub(n, sub) => {
                    let p = env.ws_root.join(n).join(sub);
                    std::fs::create_dir_all(&p).expect("create start subdir");
                    p
                }
            };
            let expected = match sc.expect {
                Expect::Root => env.ws_root.clone(),
                Expect::Ws(n) => env.ws_root.join(n),
                Expect::Unchanged => start.clone(),
            };

            let (actual, stderr) = run_in_shell(shell, &env, &start, sc.cmd);
            let actual = canon(&actual);
            let expected = canon(&expected);
            ran += 1;

            let ctx = format!(
                "shell={} scenario={:?} cmd=`wsp {}`\n  command stderr: {}",
                shell.bin,
                sc.name,
                sc.cmd.join(" "),
                if stderr.is_empty() { "<none>" } else { &stderr },
            );

            assert_eq!(
                actual,
                expected,
                "{ctx}\n  expected cwd: {}\n  actual cwd:   {}",
                expected.display(),
                actual.display(),
            );
        }
    }

    if !skipped.is_empty() {
        eprintln!(
            "note: shells not installed, skipped: {}",
            skipped.join(", ")
        );
    }

    // Locally a missing shell is a convenience skip. In CI it is a silent loss
    // of coverage for an entire dialect — exactly the blind spot that let the
    // wrapper drift in the first place — so CI sets this and demands all of
    // them. See the fish/zsh install steps in .github/workflows/ci.yml.
    if std::env::var_os("WSP_SHELL_TESTS_REQUIRE_ALL").is_some() {
        assert!(
            skipped.is_empty(),
            "WSP_SHELL_TESTS_REQUIRE_ALL is set but these shells are missing: {}. \
             Install them or unset the variable.",
            skipped.join(", ")
        );
    }

    assert!(
        ran > 0,
        "no shell was available to test; expected at least one of {:?}",
        SHELLS.iter().map(|s| s.bin).collect::<Vec<_>>()
    );
}

/// Run a command through the wrapper and return its exit status.
///
/// Separate from `run_in_shell` because the wrapper's *exit code* is a distinct
/// contract from where it leaves the shell, and both fixes in this area
/// restructured the return paths in all four dialects. A wrapper that cds
/// correctly but swallows a non-zero status silently breaks `wsp new x && ...`
/// and any script that checks the result.
fn status_in_shell(shell: &Shell, env: &Env, start: &Path, cmd: &[&str]) -> i32 {
    let load = shell.load.replace("WSPBIN", &quote(shell.quote, WSP));
    let script = format!(
        "{load}\ncd {}\nwsp {} >{null} 2>{null}\n{}",
        quote(shell.quote, &start.to_string_lossy()),
        cmd.join(" "),
        shell.print_status,
        null = shell.null,
    );

    let mut c = Command::new(shell.bin);
    apply_env(&mut c, env);
    let out = c
        .args(shell.args)
        .arg(&script)
        .output()
        .unwrap_or_else(|e| panic!("spawning {} failed: {e}", shell.bin));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or_else(|| panic!("{} printed no status\nscript:\n{script}", shell.bin));
    last.trim()
        .parse()
        .unwrap_or_else(|e| panic!("{}: unparseable status {last:?}: {e}", shell.bin))
}

/// (description, argv, expected exit status)
const EXIT_CASES: &[(&str, &[&str], i32)] = &[
    ("successful new", &["new", "w", "--empty"], 0),
    ("new with no name", &["new"], 1),
    // clap exits 2 for a usage error, not 1. Asserting the exact code (rather
    // than "non-zero") is what proves the wrapper propagates the real status
    // instead of normalizing everything to 0/1.
    ("new with a bad flag", &["new", "--nope"], 2),
    ("rm of a missing workspace", &["rm", "nope", "--force"], 1),
    // `ls` rather than `st`: these run from the workspaces root, and `st`
    // correctly fails outside a workspace.
    ("read-only ls", &["ls"], 0),
];

/// The wrapper must not swallow or invent exit codes.
#[test]
fn shell_wrapper_preserves_exit_status() {
    let mut ran = 0usize;
    for shell in SHELLS {
        if !is_installed(shell) {
            continue;
        }
        for (what, argv, want) in EXIT_CASES {
            let env = make_env();
            let got = status_in_shell(shell, &env, &env.ws_root, argv);
            ran += 1;
            assert_eq!(
                got,
                *want,
                "shell={} case={what:?} cmd=`wsp {}`: expected exit {want}, got {got}",
                shell.bin,
                argv.join(" "),
            );
        }
    }
    assert!(ran > 0, "no shell available to test exit statuses");
}

/// Run the binary directly, with `cwd` as the working directory, and return its
/// exit status. No shell, no wrapper.
fn status_direct(env: &Env, cwd: &Path, cmd: &[&str]) -> i32 {
    let mut c = Command::new(WSP);
    apply_env(&mut c, env);
    let out = c
        .args(cmd)
        .current_dir(cwd)
        .output()
        .expect("spawn wsp directly");
    out.status.code().unwrap_or(-1)
}

/// The wrapper must not change *what* a command does — only where the shell
/// ends up.
///
/// This is the general form of the bug that made `wsp rm --force --yes` fail:
/// the wrapper vacates before invoking, which silently removed the cwd that the
/// optional-positional fallback reads. Every row in the scenario table passed an
/// explicit workspace name, so nothing noticed.
///
/// Rather than enumerate argument forms — which is what missed it — this runs
/// each scenario twice from the same starting directory, once through the
/// wrapper and once against the binary directly, and requires the same exit
/// status. Any future interposition that changes what a command sees fails here
/// without anyone having to predict which form it breaks.
#[test]
fn wrapper_does_not_change_command_outcomes() {
    let mut ran = 0usize;
    for shell in SHELLS {
        if !is_installed(shell) {
            continue;
        }
        for sc in SCENARIOS {
            // Fresh fixture per side: these commands mutate state.
            let mut codes = Vec::new();
            for wrapped in [true, false] {
                let env = make_env();
                for args in sc.setup {
                    run_setup(&env, args);
                }
                let start = match sc.start {
                    Start::Root => env.ws_root.clone(),
                    Start::Ws(n) => env.ws_root.join(n),
                    Start::WsSub(n, sub) => {
                        let p = env.ws_root.join(n).join(sub);
                        std::fs::create_dir_all(&p).expect("create start subdir");
                        p
                    }
                };
                codes.push(if wrapped {
                    status_in_shell(shell, &env, &start, sc.cmd)
                } else {
                    status_direct(&env, &start, sc.cmd)
                });
            }
            ran += 1;
            assert_eq!(
                codes[0],
                codes[1],
                "shell={} scenario={:?} cmd=`wsp {}`\n  \
                 through the wrapper: exit {}\n  \
                 binary directly:     exit {}\n  \
                 The wrapper changed the command's outcome. It may only change \
                 where the shell ends up.",
                shell.bin,
                sc.name,
                sc.cmd.join(" "),
                codes[0],
                codes[1],
            );
        }
    }
    assert!(ran > 0, "no shell available for the differential test");
}
