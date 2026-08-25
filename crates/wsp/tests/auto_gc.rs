//! Auto-gc is modelled on `git gc --auto`: no daemon, piggybacking on commands
//! the user already ran. Two properties make that safe, and neither held before:
//!
//! 1. It only fires from workspace-mutating commands. git triggers auto-gc from
//!    commit/merge/rebase/fetch and never from status/log/diff; wsp fired it
//!    after *every* command, so a read-only `wsp ls` could permanently delete
//!    recoverable workspaces.
//! 2. It says what it deleted. `purge` returned a count that the caller
//!    discarded, so a purge of three workspaces printed nothing at all.

use std::path::{Path, PathBuf};
use std::process::Command;

const WSP: &str = env!("CARGO_BIN_EXE_wsp");

struct Env {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    ws_root: PathBuf,
}

fn make_env() -> Env {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("data");
    let ws_root = tmp.path().join("ws");
    std::fs::create_dir_all(data.join("wsp")).expect("data dir");
    std::fs::create_dir_all(&ws_root).expect("ws root");
    std::fs::write(
        data.join("wsp").join("config.yaml"),
        format!("workspaces_dir: '{}'\n", ws_root.display()),
    )
    .expect("config");
    Env {
        _tmp: tmp,
        data,
        ws_root,
    }
}

fn wsp(env: &Env, args: &[&str]) -> std::process::Output {
    let mut c = Command::new(WSP);
    c.env("XDG_DATA_HOME", &env.data)
        .env("HOME", env._tmp.path())
        .env("USERPROFILE", env._tmp.path())
        .args(args)
        .current_dir(&env.ws_root);
    c.output().expect("spawn wsp")
}

fn gc_dir(env: &Env) -> PathBuf {
    env.data.join("wsp").join("gc")
}

/// Age every gc entry past the retention window, and clear the hourly cooldown
/// so the next eligible command will actually attempt a purge.
fn make_everything_expired(env: &Env) {
    let dir = gc_dir(env);
    for item in std::fs::read_dir(&dir).expect("read gc dir") {
        let p = item.expect("entry").path();
        let meta = p.join(".wsp-gc.yaml");
        if let Ok(s) = std::fs::read_to_string(&meta) {
            let aged: String = s
                .lines()
                .map(|l| {
                    if l.starts_with("trashed_at:") {
                        "trashed_at: 2000-01-01T00:00:00Z".to_string()
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(&meta, aged).expect("write aged meta");
        }
    }
    let _ = std::fs::remove_file(dir.join(".gc-last"));
}

fn gc_entry_count(env: &Env) -> usize {
    std::fs::read_dir(gc_dir(env))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0)
}

fn seed_expired_entry(env: &Env, name: &str) {
    assert!(
        wsp(env, &["new", name, "--empty"]).status.success(),
        "seed: creating {name} failed"
    );
    assert!(
        wsp(env, &["rm", name, "--force", "--yes"]).status.success(),
        "seed: removing {name} failed"
    );
    make_everything_expired(env);
    assert_eq!(gc_entry_count(env), 1, "seed should leave one gc entry");
}

/// Read-only commands must not delete recoverable workspaces.
///
/// This is the regression: `wsp ls` printed "No workspaces." and destroyed a
/// recoverable workspace in the same breath.
#[test]
fn read_only_commands_do_not_purge() {
    for cmd in [
        &["ls"][..],
        &["st"][..],
        &["diff"][..],
        &["log"][..],
        // bare `wsp recover` lists, so it is read-only too
        &["recover"][..],
    ] {
        let env = make_env();
        seed_expired_entry(&env, "precious");

        let out = wsp(&env, cmd);
        let stderr = String::from_utf8_lossy(&out.stderr);

        assert_eq!(
            gc_entry_count(&env),
            1,
            "`wsp {}` purged an expired workspace; read-only commands must not \
             delete recoverable work\nstderr: {stderr}",
            cmd.join(" ")
        );
    }
}

/// A mutating command may purge — and must say so, naming what it deleted.
#[test]
fn mutating_commands_purge_and_announce() {
    let env = make_env();
    seed_expired_entry(&env, "precious");

    let out = wsp(&env, &["new", "trigger", "--empty"]);
    assert!(out.status.success(), "wsp new should succeed");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        gc_entry_count(&env),
        0,
        "a mutating command should purge the expired entry\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("gc: purged") && stderr.contains("precious"),
        "the purge must name what it deleted, or there is no record that \
         recoverable work is gone\nstderr: {stderr}"
    );
}

/// Nothing expired means nothing said. The announcement is state reporting, so
/// it must be silent exactly when there is nothing to report.
#[test]
fn nothing_expired_says_nothing() {
    let env = make_env();
    assert!(wsp(&env, &["new", "keep", "--empty"]).status.success());
    assert!(
        wsp(&env, &["rm", "keep", "--force", "--yes"])
            .status
            .success()
    );
    let _ = std::fs::remove_file(gc_dir(&env).join(".gc-last"));

    let out = wsp(&env, &["new", "another", "--empty"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("gc: purged"),
        "a fresh entry is not expired, so nothing should be announced\nstderr: {stderr}"
    );
    assert_eq!(gc_entry_count(&env), 1, "the fresh entry must survive");
}

/// `gc.retention-days = 0` means keep forever, so nothing is ever purged even
/// from a mutating command.
#[test]
fn retention_zero_never_purges() {
    let env = make_env();
    std::fs::write(
        env.data.join("wsp").join("config.yaml"),
        format!(
            "workspaces_dir: '{}'\ngc_retention_days: 0\n",
            env.ws_root.display()
        ),
    )
    .expect("config");

    seed_expired_entry(&env, "forever");
    let out = wsp(&env, &["new", "trigger", "--empty"]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        gc_entry_count(&env),
        1,
        "retention 0 means keep indefinitely\nstderr: {stderr}"
    );
}

/// Guard the gate itself: if a command is added to the allowlist, it must be one
/// that mutates workspace state. Reading the list from the source keeps this
/// honest without duplicating it.
#[test]
fn gate_list_contains_only_mutating_commands() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("main.rs"),
    )
    .expect("read main.rs");
    let gate = src
        .split("command.as_str(),")
        .nth(1)
        .and_then(|s| s.split(')').next())
        .expect("could not find the gc gate in main.rs");

    for readonly in ["ls", "st", "diff", "log", "config", "doctor", "completion"] {
        assert!(
            !gate.contains(&format!("\"{readonly}\"")),
            "`{readonly}` is read-only and must not trigger auto-gc"
        );
    }
    for mutating in ["new", "rm", "rename"] {
        assert!(
            gate.contains(&format!("\"{mutating}\"")),
            "`{mutating}` mutates workspaces and should be able to trigger auto-gc"
        );
    }
}
