//! `cd` resolves a destination without refreshing repository state.

use assert_cmd::Command;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use wsp_core::config::{Config, RepoEntry};
use wsp_core::giturl::Parsed;
use wsp_core::{mirror, workspace};

const IDENTITY: &str = "github.com/acme/widgets";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    data_home: PathBuf,
    workspace: PathBuf,
    clone: PathBuf,
    mirror: PathBuf,
    old_ref: String,
    new_ref: String,
    git_call_log: PathBuf,
    #[cfg(unix)]
    guarded_path: std::ffi::OsString,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let data_home = tmp.path().join("data");
        let data_dir = data_home.join("wsp");
        let workspaces = tmp.path().join("workspaces");
        let workspace = workspaces.join("feature");
        let clone = workspace.join("widgets");
        let source = tmp.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&data_dir).unwrap();

        git(&source, &["init", "--initial-branch=main"]);
        git(&source, &["config", "user.name", "Test"]);
        git(&source, &["config", "user.email", "test@example.com"]);
        git(&source, &["config", "commit.gpgsign", "false"]);
        fs::write(source.join("file"), "first\n").unwrap();
        git(&source, &["add", "file"]);
        git(&source, &["commit", "-m", "first"]);
        let old_ref = git(&source, &["rev-parse", "HEAD"]);

        let parsed = Parsed::from_identity(IDENTITY).unwrap();
        let mirror = mirror::dir(&data_dir.join("mirrors"), &parsed);
        fs::create_dir_all(mirror.parent().unwrap()).unwrap();
        git(
            tmp.path(),
            &[
                "clone",
                "--mirror",
                source.to_str().unwrap(),
                mirror.to_str().unwrap(),
            ],
        );
        git(
            &mirror,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        );
        git(&mirror, &["fetch", "origin"]);
        fs::create_dir_all(&workspace).unwrap();
        git(
            &workspace,
            &["clone", mirror.to_str().unwrap(), clone.to_str().unwrap()],
        );
        assert_eq!(
            git(&clone, &["rev-parse", "refs/remotes/origin/main"]),
            old_ref
        );

        fs::write(source.join("file"), "second\n").unwrap();
        git(&source, &["commit", "-am", "second"]);
        let new_ref = git(&source, &["rev-parse", "HEAD"]);
        git(&mirror, &["fetch", "origin"]);
        assert_eq!(
            git(&mirror, &["rev-parse", "refs/remotes/origin/main"]),
            new_ref
        );
        git(
            &clone,
            &["remote", "add", "wsp-mirror", mirror.to_str().unwrap()],
        );
        fs::write(clone.join(".git/FETCH_HEAD"), "sentinel\n").unwrap();

        let config = Config {
            workspaces_dir: Some(workspaces.display().to_string()),
            repos: BTreeMap::from([(
                IDENTITY.to_string(),
                RepoEntry {
                    url: "git@test.local:acme/widgets.git".to_string(),
                    added: chrono::Utc::now(),
                    setup_commands: None,
                },
            )]),
            ..Default::default()
        };
        config.save_to(&data_dir.join("config.yaml")).unwrap();
        workspace::save_metadata(
            &workspace,
            &workspace::Metadata {
                version: 0,
                name: "feature".to_string(),
                branch: "main".to_string(),
                repos: BTreeMap::from([(IDENTITY.to_string(), None)]),
                registry_urls: BTreeMap::new(),
                created: chrono::Utc::now(),
                description: None,
                last_used: None,
                created_from: None,
                dirs: BTreeMap::new(),
                config: None,
                setup_commands: BTreeMap::new(),
            },
        )
        .unwrap();

        let git_call_log = tmp.path().join("git-calls");
        #[cfg(unix)]
        let guarded_path = {
            use std::os::unix::fs::PermissionsExt;
            let bin = tmp.path().join("bin");
            fs::create_dir_all(&bin).unwrap();
            let git_shim = bin.join("git");
            fs::write(
                &git_shim,
                "#!/bin/sh\nprintf 'called\\n' >> \"$WSP_GIT_CALL_LOG\"\nexit 99\n",
            )
            .unwrap();
            fs::set_permissions(&git_shim, fs::Permissions::from_mode(0o755)).unwrap();
            std::env::join_paths(
                std::iter::once(bin)
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap()
        };
        #[cfg(unix)]
        {
            let guard_probe = std::process::Command::new("git")
                .arg("--version")
                .env("PATH", &guarded_path)
                .env("WSP_GIT_CALL_LOG", &git_call_log)
                .output()
                .unwrap();
            assert_eq!(guard_probe.status.code(), Some(99));
            assert_eq!(fs::read_to_string(&git_call_log).unwrap(), "called\n");
            fs::remove_file(&git_call_log).unwrap();
        }

        Self {
            _tmp: tmp,
            data_home,
            workspace,
            clone,
            mirror,
            old_ref,
            new_ref,
            git_call_log,
            #[cfg(unix)]
            guarded_path,
        }
    }

    fn assert_unchanged(&self) {
        assert_ne!(
            self.old_ref, self.new_ref,
            "fixture must contain a newer mirror ref"
        );
        assert_eq!(
            git(&self.clone, &["rev-parse", "refs/remotes/origin/main"]),
            self.old_ref,
            "cd must not update remote-tracking refs"
        );
        assert_eq!(
            fs::read_to_string(self.clone.join(".git/FETCH_HEAD")).unwrap(),
            "sentinel\n",
            "cd must not rewrite FETCH_HEAD"
        );
        assert_eq!(
            git(&self.clone, &["remote", "get-url", "wsp-mirror"]),
            self.mirror.display().to_string(),
            "cd must not rewrite remotes"
        );
        assert!(
            !self.git_call_log.exists(),
            "cd must not invoke git even when a mirror is available"
        );
    }
}

#[test]
fn cd_only_returns_the_workspace_path() {
    for (shell, json) in [(false, false), (true, false), (false, true), (true, true)] {
        let fixture = Fixture::new();
        let mut command = Command::cargo_bin("wsp").unwrap();
        command
            .env("XDG_DATA_HOME", &fixture.data_home)
            .env("WSP_GIT_CALL_LOG", &fixture.git_call_log)
            .current_dir(&fixture.workspace)
            .args(["cd", "feature"]);
        #[cfg(unix)]
        command.env("PATH", &fixture.guarded_path);
        if shell {
            command.env("WSP_SHELL", "1");
        } else {
            command.env_remove("WSP_SHELL");
        }
        if json {
            command.arg("--json");
        }

        let assert = command.assert().success();
        let output = assert.get_output();
        let stdout = String::from_utf8(output.stdout.clone()).unwrap();
        let stderr = String::from_utf8(output.stderr.clone()).unwrap();
        if json {
            let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
            assert_eq!(value["path"], fixture.workspace.display().to_string());
        } else {
            assert_eq!(stdout.trim_end(), fixture.workspace.display().to_string());
        }
        assert_eq!(
            stderr.contains("shell integration not active"),
            !shell,
            "unexpected stderr for shell={shell}, json={json}: {stderr}"
        );
        fixture.assert_unchanged();
    }
}
