//! End-to-end coverage for resumable `wsp sync` conflicts.
//!
//! This runs the real binary over a workspace clone and its bare mirror. It
//! proves the user workflow: a conflict pauses the command without discarding
//! Git state, then resolving and rerunning the same command completes it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Output, Stdio};
use std::time::Duration;

use wsp_core::config::{Config, Paths, RepoEntry};
use wsp_core::giturl::Parsed;
use wsp_core::mirror;
use wsp_core::workspace::{self, Metadata};

const IDENTITY: &str = "127.0.0.1/user/repo";
const WORKSPACE: &str = "feature";
const REPO_DIR: &str = "repo";
const WSP: &str = env!("CARGO_BIN_EXE_wsp");

struct Fixture {
    _tmp: tempfile::TempDir,
    daemon: Child,
    xdg_data_home: PathBuf,
    templates_dir: PathBuf,
    workspace_dir: PathBuf,
    clone_dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let output = StdCommand::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn commit(dir: &Path, content: &str, message: &str) {
    std::fs::write(dir.join("conflict.txt"), content).unwrap();
    git(dir, &["add", "conflict.txt"]);
    git(dir, &["commit", "-m", message]);
}

fn setup() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let source_dir = tmp.path().join("source");
    std::fs::create_dir(&source_dir).unwrap();
    git(&source_dir, &["init"]);
    git(&source_dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&source_dir, &["config", "user.email", "test@test.local"]);
    git(&source_dir, &["config", "user.name", "Test"]);
    git(&source_dir, &["config", "commit.gpgsign", "false"]);
    commit(&source_dir, "initial\n", "initial");
    std::fs::write(
        source_dir.join("demo.wsp.yaml"),
        "repos:\n  - url: git@test.local:user/companion.git\n",
    )
    .unwrap();
    git(&source_dir, &["add", "demo.wsp.yaml"]);
    git(&source_dir, &["commit", "-m", "add workspace template"]);

    let remote_dir = tmp.path().join("user/repo.git");
    std::fs::create_dir_all(remote_dir.parent().unwrap()).unwrap();
    git(
        tmp.path(),
        &["init", "--bare", remote_dir.to_str().unwrap()],
    );
    git(
        &source_dir,
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
    );
    git(&source_dir, &["push", "origin", "main"]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let daemon = StdCommand::new("git")
        .args([
            "daemon",
            "--reuseaddr",
            "--export-all",
            &format!("--base-path={}", tmp.path().display()),
            "--listen=127.0.0.1",
            &format!("--port={port}"),
            tmp.path().to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let address = format!("127.0.0.1:{port}").parse().unwrap();
    for _ in 0..50 {
        if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let identity_url = format!("git://127.0.0.1:{port}/user/repo.git");

    let xdg_data_home = tmp.path().join("data");
    let paths = Paths::from_dirs(&xdg_data_home.join("wsp"), &tmp.path().join("workspaces"));
    let workspace_dir = paths.workspaces_dir.join(WORKSPACE);
    std::fs::create_dir_all(&workspace_dir).unwrap();

    let clone_dir = workspace_dir.join(REPO_DIR);
    let clone_output = StdCommand::new("git")
        .args([
            "clone",
            source_dir.to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        clone_output.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&clone_output.stderr)
    );
    git(&clone_dir, &["config", "user.email", "test@test.local"]);
    git(&clone_dir, &["config", "user.name", "Test"]);
    git(&clone_dir, &["config", "commit.gpgsign", "false"]);
    git(
        &clone_dir,
        &["checkout", "-b", WORKSPACE, "--no-track", "origin/main"],
    );

    let mirror_dir = mirror::dir(
        &paths.mirrors_dir,
        &Parsed::from_identity(IDENTITY).unwrap(),
    );
    std::fs::create_dir_all(mirror_dir.parent().unwrap()).unwrap();
    let mirror_output = StdCommand::new("git")
        .args([
            "clone",
            "--mirror",
            source_dir.to_str().unwrap(),
            mirror_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        mirror_output.status.success(),
        "git clone --mirror failed: {}",
        String::from_utf8_lossy(&mirror_output.stderr)
    );
    // Use a reachable literal URL so source-provenance checks exercise the
    // same transport as production without URL rewrite rules.
    for repo in [&clone_dir, &mirror_dir] {
        git(repo, &["remote", "set-url", "origin", &identity_url]);
    }

    commit(&clone_dir, "local\n", "local change");
    commit(&source_dir, "upstream\n", "upstream change");
    git(&source_dir, &["push", "origin", "main"]);

    Config {
        workspaces_dir: Some(paths.workspaces_dir.display().to_string()),
        repos: BTreeMap::from([(
            IDENTITY.to_string(),
            RepoEntry {
                url: identity_url,
                added: chrono::Utc::now(),
                setup_commands: None,
            },
        )]),
        ..Default::default()
    }
    .save_to(&paths.config_path)
    .unwrap();
    workspace::save_metadata(
        &workspace_dir,
        &Metadata {
            version: 0,
            name: WORKSPACE.to_string(),
            branch: WORKSPACE.to_string(),
            repos: BTreeMap::from([(IDENTITY.to_string(), None)]),
            registry_urls: std::collections::BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::from([(IDENTITY.to_string(), REPO_DIR.to_string())]),
            config: None,
            setup_commands: BTreeMap::new(),
        },
    )
    .unwrap();

    Fixture {
        _tmp: tmp,
        daemon,
        xdg_data_home,
        templates_dir: paths.templates_dir,
        workspace_dir,
        clone_dir,
    }
}

fn wsp(fixture: &Fixture) -> Output {
    StdCommand::new(WSP)
        .env("XDG_DATA_HOME", &fixture.xdg_data_home)
        .env("HOME", fixture._tmp.path())
        .env("USERPROFILE", fixture._tmp.path())
        .current_dir(fixture._tmp.path())
        .stdin(Stdio::null())
        .args(["--json", "sync", WORKSPACE])
        .output()
        .unwrap()
}

#[test]
fn sync_conflict_is_resumed_by_rerunning_the_real_binary() {
    let fixture = setup();

    let paused = wsp(&fixture);
    assert_eq!(
        paused.status.code(),
        Some(2),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&paused.stdout),
        String::from_utf8_lossy(&paused.stderr)
    );
    let paused_json: serde_json::Value = serde_json::from_slice(&paused.stdout).unwrap();
    assert_eq!(paused_json["repos"][0]["status"], "paused");
    assert!(
        fixture.clone_dir.join(".git/rebase-merge").exists(),
        "the conflicting rebase must remain available to resolve"
    );

    std::fs::write(fixture.clone_dir.join("conflict.txt"), "resolved\n").unwrap();
    git(&fixture.clone_dir, &["add", "conflict.txt"]);

    let resumed = wsp(&fixture);
    assert!(
        resumed.status.success(),
        "rerun failed: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed_json: serde_json::Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed_json["repos"][0]["status"], "ok");
    assert!(
        !String::from_utf8_lossy(&resumed.stderr).contains("hint: found template"),
        "sync must not scan repos for templates"
    );
    assert!(
        !fixture.templates_dir.join("demo.yaml").exists(),
        "sync must not import repo templates"
    );
    assert!(
        !fixture.clone_dir.join(".git/rebase-merge").exists(),
        "rerunning sync must finish the resolved rebase"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.clone_dir.join("conflict.txt")).unwrap(),
        "resolved\n"
    );
    assert!(fixture.workspace_dir.exists());
}
