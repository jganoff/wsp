/// Integration tests for the mirror-propagation skip warnings.
///
/// When a workspace repo has no mirror — usually because its slug was never
/// added to the global registry — `wsp cd` used to dump a raw `git fetch` fatal
/// ("does not appear to be a git repository") that said nothing about the
/// missing registry entry. These tests run the actual binary and assert the
/// warning names the cause and the fix instead.
use assert_cmd::Command;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use wsp_core::config::{Config, RepoEntry};
use wsp_core::workspace;

const IDENTITY: &str = "github.com/acme/widgets";
const WS_NAME: &str = "my-feature";
const RAW_GIT_FATAL: &str = "does not appear to be a git repository";

struct Env {
    _tmp: tempfile::TempDir,
    xdg_data_home: PathBuf,
    ws_dir: PathBuf,
    clone_dir: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Build a workspace holding one repo clone whose mirror does not exist.
/// `registered` controls whether the repo identity is in the global registry.
fn setup(registered: bool) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let xdg_data_home = tmp.path().join("data");
    let data_dir = xdg_data_home.join("wsp");
    let workspaces_dir = tmp.path().join("workspaces");
    fs::create_dir_all(&data_dir).unwrap();
    fs::create_dir_all(&workspaces_dir).unwrap();

    let mut cfg = Config {
        workspaces_dir: Some(workspaces_dir.display().to_string()),
        ..Default::default()
    };
    if registered {
        cfg.repos.insert(
            IDENTITY.to_string(),
            RepoEntry {
                url: "git@test.local:acme/widgets.git".to_string(),
                added: chrono::Utc::now(),
                setup_commands: None,
            },
        );
    }
    cfg.save_to(&data_dir.join("config.yaml")).unwrap();

    let ws_dir = workspaces_dir.join(WS_NAME);
    let clone_dir = ws_dir.join("widgets");
    fs::create_dir_all(&clone_dir).unwrap();
    git(&clone_dir, &["init", "--initial-branch=main"]);

    let meta = workspace::Metadata {
        version: 0,
        name: WS_NAME.to_string(),
        branch: format!("test/{WS_NAME}"),
        repos: BTreeMap::from([(IDENTITY.to_string(), None)]),
        created: chrono::Utc::now(),
        description: None,
        last_used: None,
        created_from: None,
        dirs: BTreeMap::new(),
        config: None,
        setup_commands: BTreeMap::new(),
    };
    workspace::save_metadata(&ws_dir, &meta).unwrap();

    Env {
        _tmp: tmp,
        xdg_data_home,
        ws_dir,
        clone_dir,
    }
}

/// Run `wsp cd <workspace>` and return stderr.
fn run_cd(env: &Env) -> String {
    let assert = Command::cargo_bin("wsp")
        .unwrap()
        .env("XDG_DATA_HOME", &env.xdg_data_home)
        .env("NO_COLOR", "1")
        .current_dir(&env.ws_dir)
        .args(["cd", WS_NAME])
        .assert()
        .success();
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

#[test]
fn unregistered_repo_warning_names_registry_and_fix() {
    let env = setup(/* registered */ false);
    let stderr = run_cd(&env);

    assert!(
        stderr.contains(IDENTITY) && stderr.contains("not registered"),
        "stderr should name the unregistered repo, got:\n{stderr}"
    );
    assert!(
        stderr.contains("wsp doctor --fix") && stderr.contains("wsp repo rm widgets"),
        "stderr should offer both remedies, got:\n{stderr}"
    );
    assert!(
        !stderr.contains(RAW_GIT_FATAL),
        "raw git fatal should not leak, got:\n{stderr}"
    );
}

#[test]
fn registered_repo_with_missing_mirror_warns_about_the_mirror() {
    let env = setup(/* registered */ true);
    let stderr = run_cd(&env);

    assert!(
        stderr.contains(IDENTITY) && stderr.contains("mirror"),
        "stderr should name the missing mirror, got:\n{stderr}"
    );
    assert!(
        stderr.contains("wsp doctor --fix"),
        "stderr should offer a fix, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("not registered"),
        "a registered repo should not be reported as unregistered, got:\n{stderr}"
    );
    assert!(
        !stderr.contains(RAW_GIT_FATAL),
        "raw git fatal should not leak, got:\n{stderr}"
    );
}

#[test]
fn missing_clone_directory_warns_instead_of_failing_the_spawn() {
    let env = setup(/* registered */ true);
    fs::remove_dir_all(&env.clone_dir).unwrap();
    let stderr = run_cd(&env);

    assert!(
        stderr.contains("clone directory 'widgets' is missing"),
        "stderr should name the missing clone dir, got:\n{stderr}"
    );
    assert!(
        stderr.contains("wsp doctor"),
        "stderr should offer a fix, got:\n{stderr}"
    );
}
