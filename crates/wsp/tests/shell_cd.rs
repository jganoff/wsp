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

struct Shell {
    /// Executable name.
    bin: &'static str,
    /// Name passed to `wsp completion <name>`.
    completion: &'static str,
    /// Flags that disable rc/profile loading, plus the "run this string" flag.
    /// The script is appended as the final argument.
    args: &'static [&'static str],
    /// Snippet that loads the wrapper into the current shell.
    load: &'static str,
    /// Prints the current directory followed by a newline.
    print_pwd: &'static str,
}

#[cfg(unix)]
const SHELLS: &[Shell] = &[
    Shell {
        bin: "bash",
        completion: "bash",
        args: &["--noprofile", "--norc", "-c"],
        load: r#"eval "$(WSPBIN completion bash)" 2>/dev/null"#,
        print_pwd: r#"printf '%s\n' "$PWD""#,
    },
    Shell {
        bin: "zsh",
        completion: "zsh",
        // -f == NO_RCS: skip .zshenv/.zshrc entirely.
        args: &["-f", "-c"],
        load: r#"eval "$(WSPBIN completion zsh)" 2>/dev/null"#,
        print_pwd: r#"printf '%s\n' "$PWD""#,
    },
    Shell {
        bin: "fish",
        completion: "fish",
        args: &["--no-config", "-c"],
        load: r#"WSPBIN completion fish 2>/dev/null | source"#,
        print_pwd: r#"printf '%s\n' "$PWD""#,
    },
];

#[cfg(windows)]
const SHELLS: &[Shell] = &[Shell {
    bin: "pwsh",
    completion: "powershell",
    args: &["-NoProfile", "-NonInteractive", "-Command"],
    load: r#"Invoke-Expression ((& WSPBIN completion powershell) -join "`n")"#,
    print_pwd: r#"Write-Output $PWD.Path"#,
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
    /// Passes today; a regression should fail the build.
    Works,
    /// Known-broken. The test asserts the *wrong* behavior so CI stays green,
    /// and fails loudly if the behavior starts matching `expect` — at which
    /// point the row should be promoted to `Works`.
    KnownBroken(&'static str),
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
        status: Status::KnownBroken("wrapper parses argv positionally; see #105"),
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
fn run_in_shell(shell: &Shell, env: &Env, start: &Path, cmd: &[&str]) -> Option<PathBuf> {
    let bin_quoted = shell_quote(shell, WSP);
    let load = shell.load.replace("WSPBIN", &bin_quoted);
    let start_quoted = shell_quote(shell, &start.to_string_lossy());

    // Discard the command's own output; only the trailing $PWD line is read.
    let script = format!(
        "{load}\ncd {start_quoted}\nwsp {} >{null} 2>{null}\n{}",
        cmd.join(" "),
        shell.print_pwd,
        null = null_device(),
    );

    let mut c = Command::new(shell.bin);
    apply_env(&mut c, env);
    let out = match c.args(shell.args).arg(&script).output() {
        Ok(o) => o,
        // Shell not present (fish is commonly absent); skip rather than fail.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("spawning {} failed: {e}", shell.bin),
    };

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
    Some(PathBuf::from(last.trim()))
}

fn shell_quote(shell: &Shell, s: &str) -> String {
    if shell.completion == "powershell" {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn null_device() -> &'static str {
    if cfg!(windows) { "$null" } else { "/dev/null" }
}

#[test]
fn shell_wrapper_cd_behavior_matches_across_shells() {
    let mut ran = 0usize;
    let mut skipped: Vec<&str> = Vec::new();

    for shell in SHELLS {
        let mut shell_available = true;

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

            let Some(actual) = run_in_shell(shell, &env, &start, sc.cmd) else {
                shell_available = false;
                break;
            };
            ran += 1;

            let (actual, expected) = (canon(&actual), canon(&expected));
            let ctx = format!(
                "shell={} scenario={:?} cmd=`wsp {}`",
                shell.bin,
                sc.name,
                sc.cmd.join(" ")
            );

            match sc.status {
                Status::Works => assert_eq!(
                    actual,
                    expected,
                    "{ctx}\n  expected cwd: {}\n  actual cwd:   {}",
                    expected.display(),
                    actual.display(),
                ),
                Status::KnownBroken(why) => assert_ne!(
                    actual,
                    expected,
                    "{ctx}\n  This scenario is marked KnownBroken ({why}) but now \
                     produces the correct directory ({}). The fix landed — promote \
                     this row to Status::Works.",
                    expected.display(),
                ),
            }
        }

        if !shell_available {
            skipped.push(shell.bin);
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
