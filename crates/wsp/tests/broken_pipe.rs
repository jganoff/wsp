//! A reader that stops reading must not turn into a panic.
//!
//! `wsp exec ... | head -1` used to print a Rust panic dump and exit 101. Rust
//! ignores SIGPIPE, so the write returns EPIPE and the `print!` family panics.
//!
//! This is a behavioural test on purpose. The fix in `main.rs` recognises the
//! panic by the message std produces (`failed printing to stdout: ...`), which
//! is a string std owns and could reword. Asserting on `is_closed_pipe_panic`
//! directly would keep passing after such a reword while the binary went back to
//! dumping panics — the same trap `tests/shell_cd.rs` documents for the shell
//! wrapper. So this runs the real binary and looks at what a user would see.
//!
//! `exec` is the subject because it is the only command that can reach the bug
//! offline: it writes a block per repo with a subprocess in between, so a reader
//! has time to leave mid-stream. Anything that writes once fits inside the pipe
//! buffer and never learns the reader is gone.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const WSP: &str = env!("CARGO_BIN_EXE_wsp");

/// An isolated data dir and workspaces dir under one tempdir, so a lookup that
/// ignores `XDG_DATA_HOME` still cannot reach the developer's real workspaces.
struct Env {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    workspaces: PathBuf,
}

fn make_env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let workspaces = tmp.path().join("workspaces");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&workspaces).unwrap();
    Env {
        _tmp: tmp,
        data,
        workspaces,
    }
}

fn wsp(env: &Env) -> Command {
    let mut cmd = Command::new(WSP);
    // HOME too: a lookup that ignores XDG_DATA_HOME still lands in the tempdir.
    cmd.env("XDG_DATA_HOME", &env.data)
        .env("HOME", env._tmp.path())
        .env("USERPROFILE", env._tmp.path())
        .current_dir(env._tmp.path());
    cmd
}

fn run(env: &Env, args: &[&str]) {
    let out = wsp(env).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "setup `wsp {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A workspace whose repos exist as plain directories.
///
/// `exec` only needs somewhere to run and a metadata entry naming it, so the
/// repos are `mkdir`s and the entries are appended to the `.wsp.yaml` that
/// `wsp new --empty` just wrote. That keeps this offline: cloning anything, even
/// a local path, is not something the registry supports (see #91).
fn workspace_with_repos(env: &Env, name: &str, repos: &[&str]) -> PathBuf {
    run(
        env,
        &[
            "config",
            "set",
            "workspaces-dir",
            env.workspaces.to_str().unwrap(),
            "--global",
        ],
    );
    run(env, &["new", name, "--empty"]);

    let ws_dir = env.workspaces.join(name);
    let meta_path = ws_dir.join(".wsp.yaml");
    let meta = std::fs::read_to_string(&meta_path).unwrap();
    assert!(
        meta.contains("repos: {}"),
        "expected an empty repo map to replace, got:\n{meta}"
    );

    let mut entries = String::from("repos:\n");
    for repo in repos {
        std::fs::create_dir_all(ws_dir.join(repo)).unwrap();
        entries.push_str(&format!(
            "  test.local/u/{repo}:\n    upstream_url: git@test.local:u/{repo}.git\n"
        ));
    }
    std::fs::write(&meta_path, meta.replace("repos: {}\n", &entries)).unwrap();
    ws_dir
}

/// Read one line, then drop the pipe — the `head -1` of the standard library.
///
/// Returns the exit status and stderr. Reading a line first matters: it proves
/// the child got as far as writing, so the writes that follow are the ones that
/// meet a closed pipe.
fn first_line_then_close(mut cmd: Command) -> (Option<i32>, String) {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    drop(reader);

    let out = child.wait_with_output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The headline: no panic, and a clean exit.
///
/// `git` rather than `echo` as the child: `echo` is a shell builtin with no
/// executable on Windows, so `Command::new("echo")` fails to spawn there. wsp
/// already requires git, so it is the one program guaranteed on every platform.
///
/// Not asserted, because it is a separate and narrower bug: in non-`--json` mode
/// the child inherits our stdout, so it is killed by SIGPIPE too, and wsp still
/// reports that as `error: exit status -1` on stderr. Suppressing it needs the
/// child's signal, which is unix-only, and `exit_code` is part of the `--json`
/// contract. Left alone deliberately rather than missed.
#[test]
fn exec_exits_quietly_when_the_reader_stops() {
    let env = make_env();
    workspace_with_repos(&env, "ws", &["alpha", "beta", "gamma"]);

    let mut cmd = wsp(&env);
    cmd.args(["exec", "ws", "--", "git", "--version"]);
    let (code, stderr) = first_line_then_close(cmd);

    assert!(
        !stderr.contains("panicked"),
        "a reader that stopped produced a panic dump:\n{stderr}"
    );
    assert!(
        !stderr.contains("Broken pipe") && !stderr.contains("os error"),
        "a reader that stopped leaked a raw io error:\n{stderr}"
    );
    assert_eq!(
        code,
        Some(0),
        "expected a quiet exit 0, got {code:?}; stderr:\n{stderr}"
    );
}

/// The fixture has to actually reach the bug, or the test above proves nothing.
///
/// Without a reader leaving mid-stream there is no EPIPE to survive, and a
/// future `exec` that buffered its whole output would silently make
/// `exec_exits_quietly_when_the_reader_stops` vacuous. Reading everything must
/// still produce a block per repo, which is what creates the gap.
#[test]
fn exec_writes_one_block_per_repo() {
    let env = make_env();
    workspace_with_repos(&env, "ws", &["alpha", "beta", "gamma"]);

    let out = wsp(&env)
        .args(["exec", "ws", "--", "git", "--version"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    for repo in ["alpha", "beta", "gamma"] {
        assert!(
            stdout.contains(&format!("==> [{repo}]")),
            "no block for {repo} in:\n{stdout}"
        );
    }
    assert!(out.status.success(), "exec failed: {stdout}");
}

/// Tables take the `io::Write` path, where a closed pipe is an error rather than
/// a panic. Same situation, so it must reach the same quiet exit.
///
/// `ls` output is far smaller than a pipe buffer, so this cannot fail today —
/// the writes land before the reader can leave. It is here so that the day
/// `ls` grows paging, or a workspace list outgrows 64KB, the `io::Write` half of
/// the fix is already covered rather than discovered.
#[test]
fn ls_exits_quietly_when_the_reader_stops() {
    let env = make_env();
    for name in ["one", "two", "three"] {
        workspace_with_repos(&env, name, &["alpha"]);
    }

    let mut cmd = wsp(&env);
    cmd.arg("ls");
    let (code, stderr) = first_line_then_close(cmd);

    assert!(
        !stderr.contains("panicked"),
        "ls produced a panic dump:\n{stderr}"
    );
    assert_eq!(code, Some(0), "expected exit 0, got {code:?}:\n{stderr}");
}
