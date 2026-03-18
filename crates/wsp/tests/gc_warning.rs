/// Integration tests for gc warning banner on stderr.
///
/// The gc warning is emitted by four call sites: diff.rs, log.rs, status.rs,
/// repo_list.rs. Each calls `gc::check_workspace` and passes the result to
/// `print_gc_warning`. These tests run the actual binary from inside a gc'd
/// workspace directory to ensure none of those sites silently drop the warning.
use assert_cmd::Command;
use std::fs;
use wsp_core::config::Paths;
use wsp_core::gc;
use wsp_core::workspace;

fn test_paths(tmp: &std::path::Path) -> Paths {
    Paths {
        config_path: tmp.join("config.yaml"),
        mirrors_dir: tmp.join("mirrors"),
        gc_dir: tmp.join("gc"),
        templates_dir: tmp.join("templates"),
        workspaces_dir: tmp.join("workspaces"),
    }
}

/// Create a minimal workspace (no repos) and move it to gc.
/// Returns the path to the gc'd workspace directory (where .wsp-gc.yaml lives).
fn setup_gcd_workspace(paths: &Paths, name: &str) -> std::path::PathBuf {
    let ws_dir = paths.workspaces_dir.join(name);
    fs::create_dir_all(&ws_dir).unwrap();

    let meta = workspace::Metadata {
        version: 0,
        name: name.to_string(),
        branch: format!("test/{name}"),
        repos: std::collections::BTreeMap::new(),
        created: chrono::Utc::now(),
        description: None,
        last_used: None,
        created_from: None,
        dirs: std::collections::BTreeMap::new(),
        config: None,
    };
    workspace::save_metadata(&ws_dir, &meta).unwrap();

    gc::move_to_gc(paths, name, &format!("test/{name}")).unwrap();

    // Filter to entries starting with "<name>__" so the helper is safe even
    // if gc_dir contains multiple entries from different workspaces.
    let prefix = format!("{name}__");
    let mut entries: Vec<_> = fs::read_dir(&paths.gc_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    entries.sort();
    entries.pop().unwrap()
}

/// Shared setup: returns a (TempDir, gc_ws_dir) pair.
/// TempDir must be held for the duration of the test to keep the temp path alive.
fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = test_paths(tmp.path());
    let gc_ws_dir = setup_gcd_workspace(&paths, "my-feature");
    (tmp, gc_ws_dir)
}

fn wsp_cmd(data_dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("wsp").unwrap();
    cmd.env("XDG_DATA_HOME", data_dir);
    cmd.env("NO_COLOR", "1");
    cmd
}

#[test]
fn gc_warning_appears_on_stderr_for_wsp_st() {
    let (tmp, gc_ws_dir) = setup();
    wsp_cmd(tmp.path())
        .current_dir(&gc_ws_dir)
        .arg("st")
        .assert()
        .success()
        .stderr(predicates::str::contains("WORKSPACE REMOVED"));
}

#[test]
fn gc_warning_appears_on_stderr_for_wsp_diff() {
    let (tmp, gc_ws_dir) = setup();
    wsp_cmd(tmp.path())
        .current_dir(&gc_ws_dir)
        .arg("diff")
        .assert()
        .success()
        .stderr(predicates::str::contains("WORKSPACE REMOVED"));
}

#[test]
fn gc_warning_appears_on_stderr_for_wsp_log() {
    let (tmp, gc_ws_dir) = setup();
    wsp_cmd(tmp.path())
        .current_dir(&gc_ws_dir)
        .arg("log")
        .assert()
        .success()
        .stderr(predicates::str::contains("WORKSPACE REMOVED"));
}

#[test]
fn gc_warning_appears_on_stderr_for_wsp_repo_ls() {
    let (tmp, gc_ws_dir) = setup();
    wsp_cmd(tmp.path())
        .current_dir(&gc_ws_dir)
        .args(["repo", "ls"])
        .assert()
        .success()
        .stderr(predicates::str::contains("WORKSPACE REMOVED"));
}
