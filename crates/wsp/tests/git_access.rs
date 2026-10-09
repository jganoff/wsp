//! Real-binary observations of opt-in access probes and their isolation policy.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use wsp_core::config::{Config, RepoEntry};

const IDENTITY: &str = "github.com/fixture/remote";
const URL: &str = "https://github.com/fixture/remote";

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    mirror: PathBuf,
    clone: PathBuf,
    workspace: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    real_git: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("git"))
            .find(|path| path.is_file())
            .expect("git must be installed");
        let bin = root.path().join("bin");
        let log = root.path().join("access-log");
        let workspace = root.path().join("workspace");
        let clone = workspace.join("remote");
        let mirror = root
            .path()
            .join("data/wsp/mirrors/github.com/fixture/remote.git");
        let source = root.path().join("source.git");
        let config = root.path().join("data/wsp/config.yaml");
        for dir in [&bin, &workspace, mirror.parent().unwrap()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let fixture = Self {
            root,
            config,
            mirror,
            clone,
            workspace,
            bin,
            log,
            real_git,
        };
        fixture.git(&["init", "--quiet", "--bare", source.to_str().unwrap()]);
        fixture.git(&[
            "init",
            "--quiet",
            "--bare",
            fixture.mirror.to_str().unwrap(),
        ]);
        fixture.git(&["init", "--quiet", fixture.clone.to_str().unwrap()]);
        fixture.git(&[
            "-C",
            fixture.clone.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            URL,
        ]);
        std::fs::write(fixture.workspace.join(".wsp.yaml"), format!(
            "name: access-fixture\nbranch: fixture/access\nrepos:\n  {IDENTITY}: null\ncreated: 2026-01-01T00:00:00Z\n"
        )).unwrap();
        Config {
            branch_prefix: Some("fixture".into()),
            workspaces_dir: Some(fixture.root.path().join("workspaces").display().to_string()),
            repos: BTreeMap::from([(
                IDENTITY.into(),
                RepoEntry {
                    url: URL.into(),
                    added: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                    setup_commands: None,
                },
            )]),
            hints: Some(false),
            ..Default::default()
        }
        .save_to(&fixture.config)
        .unwrap();
        std::fs::write(
            fixture.root.path().join("gitconfig"),
            format!(
                "[url \"file://{}\"]\n\tinsteadOf = {URL}\n",
                source.display()
            ),
        )
        .unwrap();
        let wrapper = fixture.bin.join("git");
        std::fs::write(
            &wrapper,
            r#"#!/bin/sh
for argument do
  if [ "$argument" = ls-remote ]; then
    tty=absent
    if (exec 3<>/dev/tty) 2>"$WSP_ACCESS_TTY_ERROR"; then tty=present; fi
    printf '%s\n' "cwd=$PWD|prompt=$GIT_TERMINAL_PROMPT|tty=$tty|args=$*" >> "$WSP_ACCESS_LOG"
    if [ "$WSP_ACCESS_RESULT" = failed ]; then
      printf '%s\n' 'fatal: fixture refused https://user:private@example.test/repo?key=private' >&2
      exit 19
    fi
  fi
done
exec "$WSP_ACCESS_REAL_GIT" "$@"
"#,
        )
        .unwrap();
        std::fs::set_permissions(wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        fixture
    }

    fn git(&self, args: &[&str]) {
        let result = Command::new(&self.real_git)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn command(&self, program: &Path, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        let mut paths = vec![self.bin.clone()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        command
            .current_dir(cwd)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("HOME", self.root.path())
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.path().join("gitconfig"))
            .env("GIT_TERMINAL_PROMPT", "1")
            .env("WSP_ACCESS_LOG", &self.log)
            .env("WSP_ACCESS_TTY_ERROR", self.root.path().join("tty-error"))
            .env("WSP_ACCESS_REAL_GIT", &self.real_git)
            .env_remove("WSP_SHELL")
            .env_remove("WSP_CD_FILE")
            .env_remove("WSP_PWD")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str], cwd: &Path, failed: bool) -> Output {
        self.command(Path::new(env!("CARGO_BIN_EXE_wsp")), cwd)
            .args(args)
            .env(
                "WSP_ACCESS_RESULT",
                if failed { "failed" } else { "success" },
            )
            .output()
            .unwrap()
    }

    fn observations(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn clear_log(&self) {
        std::fs::write(&self.log, "").unwrap();
    }
}

fn access_checks(output: &Output) -> Vec<Value> {
    assert!(
        !output.stdout.contains(&0x1b),
        "JSON contains terminal escapes"
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON: {error}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["check"] == "git-access")
        .cloned()
        .collect()
}

#[test]
fn git_access_is_opt_in_and_preserves_configuration() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.config).unwrap();
    let plain = fixture.run(&["doctor", "--json"], fixture.root.path(), false);
    assert!(access_checks(&plain).is_empty());
    let guide = fixture.run(&["setup"], fixture.root.path(), false);
    assert!(String::from_utf8_lossy(&guide.stderr).contains("requires an interactive terminal"));
    assert!(
        fixture.observations().is_empty(),
        "ordinary setup/doctor accessed a remote"
    );
    assert_eq!(std::fs::read(&fixture.config).unwrap(), before);

    for command in ["setup", "doctor"] {
        for failed in [false, true] {
            fixture.clear_log();
            let output = fixture.run(
                &[
                    command,
                    "--check-access",
                    "--git-progress",
                    "native",
                    "--json",
                ],
                fixture.root.path(),
                failed,
            );
            let checks = access_checks(&output);
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0]["scope"], format!("registry/{IDENTITY}"));
            assert_eq!(
                checks[0]["details"]["result"],
                if failed { "failed" } else { "succeeded" }
            );
            assert_eq!(checks[0]["details"]["mode"], "parallel");
            assert_eq!(checks[0]["details"]["timeout_seconds"], 15);
            if failed {
                assert!(!output.status.success());
                assert_eq!(checks[0]["details"]["cause"], "unknown");
                assert!(
                    checks[0]["message"]
                        .as_str()
                        .unwrap()
                        .contains("--git-progress native")
                );
                assert!(!String::from_utf8_lossy(&output.stdout).contains("private"));
                assert!(!String::from_utf8_lossy(&output.stderr).contains("user:private"));
            } else if command == "setup" {
                assert!(
                    output.status.success(),
                    "setup access checks failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let calls = fixture.observations();
            assert_eq!(calls.len(), 1, "probe retried: {calls:?}");
            assert!(
                calls[0].contains("prompt=0"),
                "probe allowed terminal prompting: {calls:?}"
            );
            assert!(
                calls[0].starts_with(&format!(
                    "cwd={}|",
                    fixture.mirror.canonicalize().unwrap().display()
                )),
                "wrong registry cwd: {calls:?}"
            );
            assert_eq!(std::fs::read(&fixture.config).unwrap(), before);
            if command == "setup" {
                assert!(
                    !String::from_utf8_lossy(&output.stderr)
                        .contains("requires an interactive terminal")
                );
            }
        }
    }
}

#[test]
fn git_access_uses_workspace_origin_from_its_clone_directory() {
    let fixture = Fixture::new();
    for command in ["setup", "doctor"] {
        fixture.clear_log();
        let output = fixture.run(
            &[command, "--check-access", "--json"],
            &fixture.workspace,
            false,
        );
        let checks = access_checks(&output);
        assert_eq!(
            checks.len(),
            2,
            "must observe both registry and workspace access: {checks:?}"
        );
        assert!(
            checks
                .iter()
                .all(|check| check["details"]["result"] == "succeeded")
        );
        let calls = fixture.observations();
        assert_eq!(calls.len(), 2, "wrong probe count: {calls:?}");
        assert!(
            calls.iter().any(|line| line.starts_with(&format!(
                "cwd={}|",
                fixture.clone.canonicalize().unwrap().display()
            )) && line.ends_with(" origin")),
            "workspace probe did not use clone origin: {calls:?}"
        );
    }
}

#[test]
fn git_access_detaches_from_a_real_terminal_even_when_native_is_selected() {
    let fixture = Fixture::new();
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let in_terminal = |invocation: &str| {
        let mut command = fixture.command(Path::new("script"), fixture.root.path());
        #[cfg(target_os = "macos")]
        command.args(["-q", "/dev/null", "/bin/sh", "-c", invocation]);
        #[cfg(not(target_os = "macos"))]
        command.args(["-q", "-e", "-c", invocation, "/dev/null"]);
        // Keep the PTY input open until script observes its child exit.
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _input = child.stdin.take().unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "terminal fixture failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    };
    // A direct Git negative control proves script really supplies /dev/tty;
    // absence in the product probe must come from the isolation policy.
    in_terminal(&format!("git ls-remote {} > /dev/null", quote(URL)));
    let native = fixture.observations();
    assert_eq!(native.len(), 1);
    assert!(
        native[0].contains("tty=present"),
        "terminal fixture did not supply /dev/tty: {native:?}; error={}",
        std::fs::read_to_string(fixture.root.path().join("tty-error")).unwrap_or_default()
    );
    assert!(
        native[0].contains("prompt=1"),
        "native negative control did not preserve the environment: {native:?}"
    );
    fixture.clear_log();
    let stdout = fixture.root.path().join("stdout.json");
    let invocation = format!(
        "{} setup --check-access --git-progress native --json > {}",
        quote(env!("CARGO_BIN_EXE_wsp")),
        quote(stdout.to_str().unwrap())
    );
    in_terminal(&invocation);
    let calls = fixture.observations();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].contains("tty=absent"),
        "probe retained the controlling terminal: {calls:?}"
    );
    let value: Value = serde_json::from_slice(&std::fs::read(stdout).unwrap()).unwrap();
    assert_eq!(value["checks"][0]["details"]["result"], "succeeded");
}

#[test]
fn git_access_missing_mirror_uses_a_neutral_directory() {
    let fixture = Fixture::new();
    std::fs::rename(
        &fixture.mirror,
        fixture.root.path().join("mirror-saved.git"),
    )
    .unwrap();
    let unrelated = fixture.root.path().join("unrelated");
    fixture.git(&["init", "--quiet", unrelated.to_str().unwrap()]);
    let before = std::fs::read(&fixture.config).unwrap();
    let output = fixture.run(&["setup", "--check-access", "--json"], &unrelated, false);
    let checks = access_checks(&output);
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0]["details"]["result"], "succeeded");
    let calls = fixture.observations();
    assert_eq!(calls.len(), 1);
    assert!(
        !calls[0].starts_with(&format!(
            "cwd={}|",
            unrelated.canonicalize().unwrap().display()
        )),
        "unrelated checkout contributed probe context: {calls:?}"
    );
    assert!(
        !fixture.mirror.exists(),
        "probe recreated the missing mirror"
    );
    assert_eq!(std::fs::read(&fixture.config).unwrap(), before);
}
