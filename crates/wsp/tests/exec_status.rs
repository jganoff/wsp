//! How `wsp exec` reports a child that did not exit normally.
//!
//! Unix only: signals do not exist on Windows, so there is nothing to report
//! there and `signal` is never set.
//!
//! Behavioural rather than unit tests. The mapping from a raw wait status to
//! "killed by SIGTERM" runs through `ExitStatus`, our own formatting, and the
//! JSON encoder, and a unit test on any one of those would keep passing while
//! the binary reported something else -- the trap `tests/shell_cd.rs` documents
//! for the shell wrapper.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

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

/// `sh -c` so the child can signal itself. Needs a real workspace with repos so
/// exec has something to iterate.
fn exec_in_fixture(env: &Env, script: &str, json: bool) -> (String, String) {
    let mut cmd = wsp(env);
    cmd.arg("exec").arg("ws");
    if json {
        cmd.arg("--json");
    }
    cmd.args(["--", "sh", "-c", script]);
    let out = cmd.output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A signal is reported by name, because a number sends the reader to a table.
#[test]
fn a_signalled_child_is_reported_by_signal_name() {
    let env = make_env();
    workspace_with_repos(&env, "ws", &["alpha"]);

    let (_, stderr) = exec_in_fixture(&env, "kill -TERM $$", false);
    assert!(
        stderr.contains("killed by SIGTERM"),
        "expected the signal named, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("exit status"),
        "a signalled child did not choose an exit status, so none should be \
         quoted at it:\n{stderr}"
    );
}

/// An ordinary failure still reports its code, and claims no signal.
#[test]
fn an_ordinary_failure_still_reports_its_exit_status() {
    let env = make_env();
    workspace_with_repos(&env, "ws", &["alpha"]);

    let (_, stderr) = exec_in_fixture(&env, "exit 3", false);
    assert!(
        stderr.contains("exit status 3"),
        "expected the exit status, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("killed by"),
        "nothing killed this child:\n{stderr}"
    );
}

/// The `--json` half: `signal` is what makes the `-1` in `exit_code`
/// unambiguous, so it has to actually be there.
#[test]
fn json_carries_the_signal_beside_the_sentinel() {
    let env = make_env();
    workspace_with_repos(&env, "ws", &["alpha"]);

    let (stdout, _) = exec_in_fixture(&env, "kill -TERM $$", true);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let repo = &v["repos"][0];

    assert_eq!(repo["signal"], 15, "expected SIGTERM in `signal`: {repo}");
    assert_eq!(
        repo["exit_code"], -1,
        "a signalled child chose no exit code: {repo}"
    );
    assert_eq!(
        repo["ok"], false,
        "a signalled child did not succeed: {repo}"
    );
}

/// And is absent otherwise, so its presence alone answers "was it signalled".
#[test]
fn json_omits_the_signal_when_the_child_exited() {
    let env = make_env();
    workspace_with_repos(&env, "ws", &["alpha"]);

    let (stdout, _) = exec_in_fixture(&env, "exit 3", true);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let repo = &v["repos"][0];

    assert_eq!(repo["exit_code"], 3);
    assert!(
        repo.get("signal").is_none(),
        "a child that exited normally has no signal: {repo}"
    );
}
