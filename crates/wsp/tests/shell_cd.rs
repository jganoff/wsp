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
        quote: Quote::Posix,
    },
    Shell {
        bin: "zsh",
        // -f == NO_RCS: skip .zshenv/.zshrc entirely.
        args: &["-f", "-c"],
        load: r#"eval "$(WSPBIN completion zsh)" 2>/dev/null"#,
        print_pwd: r#"printf '%s\n' "$PWD""#,
        null: "/dev/null",
        quote: Quote::Posix,
    },
    Shell {
        bin: "fish",
        args: &["--no-config", "-c"],
        load: r#"WSPBIN completion fish 2>/dev/null | source"#,
        print_pwd: r#"printf '%s\n' "$PWD""#,
        null: "/dev/null",
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

/// Whether the scenario currently holds.
enum Status {
    /// `expect` holds today; a regression should fail the build.
    Works,
    /// Known-broken: the wrapper leaves the shell where it started instead of
    /// reaching `expect`. Asserted *exactly* rather than as "not `expect`", so
    /// the row fails loudly whether the bug gets fixed or gets worse — a bare
    /// `assert_ne!` would also pass if the fixture stopped being created or the
    /// wrapper cd'd somewhere unrelated.
    StaysPut(&'static str),
}

struct Scenario {
    name: &'static str,
    /// Run with the binary directly (no shell) to build the fixture.
    setup: &'static [&'static [&'static str]],
    start: Start,
    /// The `wsp ...` invocation under test, as it would be typed.
    cmd: &'static [&'static str],
    expect: Expect,
    status: Status,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "new cds into the new workspace",
        setup: &[],
        start: Start::Root,
        cmd: &["new", "w", "--empty"],
        expect: Expect::Ws("w"),
        status: Status::Works,
    },
    Scenario {
        // The regression that motivated this file: `create` is a visible alias
        // for `new`, and the wrapper used to dispatch on the literal name only.
        name: "create alias cds like new",
        setup: &[],
        start: Start::Root,
        cmd: &["create", "w", "--empty"],
        expect: Expect::Ws("w"),
        status: Status::Works,
    },
    Scenario {
        // The wrapper reads the workspace name positionally, so a leading flag
        // is mistaken for the name and it cds to `<root>/--empty`, which fails.
        // The workspace *is* created; only the cd is wrong.
        name: "new with a leading flag still cds",
        setup: &[],
        start: Start::Root,
        cmd: &["new", "--empty", "w"],
        expect: Expect::Ws("w"),
        status: Status::StaysPut("wrapper parses argv positionally; see #105"),
    },
    Scenario {
        name: "cd enters the workspace",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Root,
        cmd: &["cd", "w"],
        expect: Expect::Ws("w"),
        status: Status::Works,
    },
    Scenario {
        // Guards the "does this cd for every command?" worry: read-only
        // commands must leave the shell where it is.
        name: "st does not move the shell",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Ws("w"),
        cmd: &["st"],
        expect: Expect::Unchanged,
        status: Status::Works,
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
        status: Status::Works,
    },
    Scenario {
        name: "remove alias escapes like rm",
        setup: &[&["new", "w", "--empty"]],
        start: Start::Ws("w"),
        cmd: &["remove", "w", "--force"],
        expect: Expect::Root,
        status: Status::Works,
    },
    Scenario {
        name: "recover cds into the restored workspace",
        setup: &[&["new", "w", "--empty"], &["rm", "w", "--force"]],
        start: Start::Root,
        cmd: &["recover", "w"],
        expect: Expect::Ws("w"),
        status: Status::Works,
    },
    Scenario {
        // `recover ls` is a subcommand, not a workspace name — it must not be
        // treated as one and cd'd into.
        name: "recover ls does not move the shell",
        setup: &[&["new", "w", "--empty"], &["rm", "w", "--force"]],
        start: Start::Root,
        cmd: &["recover", "ls"],
        expect: Expect::Unchanged,
        status: Status::Works,
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

fn make_env() -> Env {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let data = tmp.path().join("data");
    let ws_root = tmp.path().join("ws");
    std::fs::create_dir_all(data.join("wsp")).expect("create data dir");
    std::fs::create_dir_all(&ws_root).expect("create ws root");
    // Point workspaces_dir at the tempdir. Without this the binary would fall
    // back to $HOME/dev/workspaces.
    std::fs::write(
        data.join("wsp").join("config.yaml"),
        format!("workspaces_dir: {}\n", ws_root.display()),
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

/// Returns `None` if the shell is not installed on this machine.
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

fn run_in_shell(shell: &Shell, env: &Env, start: &Path, cmd: &[&str]) -> PathBuf {
    let load = shell.load.replace("WSPBIN", &quote(shell.quote, WSP));

    // Scenario args are static identifiers and flags, so joining them without
    // per-arg quoting is safe here. Discard the command's own output; only the
    // trailing $PWD line is read.
    let script = format!(
        "{load}\ncd {}\nwsp {} >{null} 2>{null}\n{}",
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
    PathBuf::from(last.trim())
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
            };
            let expected = match sc.expect {
                Expect::Root => env.ws_root.clone(),
                Expect::Ws(n) => env.ws_root.join(n),
                Expect::Unchanged => start.clone(),
            };

            let actual = canon(&run_in_shell(shell, &env, &start, sc.cmd));
            ran += 1;

            let ctx = format!(
                "shell={} scenario={:?} cmd=`wsp {}`",
                shell.bin,
                sc.name,
                sc.cmd.join(" ")
            );

            match sc.status {
                Status::Works => assert_eq!(
                    actual,
                    canon(&expected),
                    "{ctx}\n  expected cwd: {}\n  actual cwd:   {}",
                    canon(&expected).display(),
                    actual.display(),
                ),
                Status::StaysPut(why) => assert_eq!(
                    actual,
                    canon(&start),
                    "{ctx}\n  Marked StaysPut ({why}), so the shell was expected to \
                     remain at {}. If it is now at {}, the fix landed — promote this \
                     row to Status::Works.",
                    canon(&start).display(),
                    canon(&expected).display(),
                ),
            }
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
