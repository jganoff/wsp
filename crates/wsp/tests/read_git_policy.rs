//! Required Git captures keep handled probe failures quiet and honor repo policy.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use wsp_core::config::{Config, ProgressConfig};
use wsp_core::git_policy::Mode;
use wsp_core::workspace::{self, Metadata};

const IDENTITY: &str = "example.test/team/repo";

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    clone: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("gitconfig");
        std::fs::write(
            &config,
            "[user]\n name = Fixture\n email = fixture@example.test\n[commit]\n gpgsign = false\n",
        )
        .unwrap();
        let source = root.path().join("source");
        let workspace = root.path().join("workspaces/feature");
        let clone = workspace.join("repo");
        let fixture = Self {
            root,
            config,
            clone,
            workspace,
        };
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&fixture.workspace).unwrap();
        fixture.git(&source, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(source.join("file"), "before\n").unwrap();
        fixture.git(&source, &["add", "file"]);
        fixture.git(&source, &["commit", "--quiet", "-m", "initial"]);
        fixture.git(
            fixture.root.path(),
            &[
                "clone",
                "--quiet",
                source.to_str().unwrap(),
                fixture.clone.to_str().unwrap(),
            ],
        );
        fixture.git(
            &fixture.clone,
            &["checkout", "--quiet", "-b", "feature", "--no-track"],
        );
        std::fs::write(fixture.clone.join("file"), "committed\n").unwrap();
        fixture.git(&fixture.clone, &["add", "file"]);
        fixture.git(
            &fixture.clone,
            &["commit", "--quiet", "-m", "workspace change"],
        );
        std::fs::write(fixture.clone.join("file"), "after\n").unwrap();
        fixture.configure(Mode::Parallel, None);
        workspace::save_metadata(
            &fixture.workspace,
            &Metadata {
                version: 0,
                name: "feature".into(),
                branch: "feature".into(),
                repos: BTreeMap::from([(IDENTITY.into(), None)]),
                registry_urls: BTreeMap::new(),
                created: chrono::Utc::now(),
                description: None,
                last_used: None,
                created_from: None,
                dirs: BTreeMap::from([(IDENTITY.into(), "repo".into())]),
                config: None,
                setup_commands: BTreeMap::new(),
            },
        )
        .unwrap();
        fixture
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn configure(&self, global: Mode, repository: Option<Mode>) {
        Config {
            workspaces_dir: Some(self.root.path().join("workspaces").display().to_string()),
            hints: Some(false),
            progress: Some(ProgressConfig {
                mode: Some(global),
                repos: repository
                    .map(|mode| BTreeMap::from([(IDENTITY.into(), mode)]))
                    .unwrap_or_default(),
            }),
            ..Default::default()
        }
        .save_to(&self.root.path().join("data/wsp/config.yaml"))
        .unwrap();
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.workspace)
            .env("HOME", self.root.path())
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("WSP_READ_BINARY", env!("CARGO_BIN_EXE_wsp"))
            .env("TERM", "xterm-256color")
            .env("LC_ALL", "C")
            .env_remove("WSP_SHELL")
            .env_remove("WSP_PWD")
            .env_remove("WSP_CD_FILE");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_wsp"))
            .args(args)
            .output()
            .unwrap()
    }
}

#[test]
fn handled_upstream_probe_failures_stay_quiet_in_successful_reads() {
    let fixture = Fixture::new();
    for command in ["diff", "log"] {
        for json in [false, true] {
            let mut args = vec![command];
            if json {
                args.push("--json");
            }
            let output = fixture.run(&args);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{args:?}: {stderr}");
            assert!(
                !stderr.contains("fatal:") && !stderr.contains("no upstream configured"),
                "handled probe leaked: {stderr}"
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            if json {
                let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert!(value.is_object());
            }
            assert!(
                stdout.contains(if command == "diff" {
                    "after"
                } else {
                    "workspace change"
                }),
                "read result lost: {stdout}"
            );
        }
    }
}

#[test]
fn repository_policy_reaches_terminal_dependent_read_helpers() {
    for action in ["diff", "log -- -p --ext-diff -1", "st"] {
        for (global, repository, invocation, terminal) in [
            (Mode::Parallel, Some(Mode::Native), None, true),
            (Mode::Native, Some(Mode::Parallel), None, false),
            (Mode::Parallel, Some(Mode::Native), Some("parallel"), false),
            (Mode::Native, Some(Mode::Parallel), Some("native"), true),
        ] {
            let fixture = Fixture::new();
            fixture.configure(global, repository);
            let helper = fixture.root.path().join("external-diff");
            let result = fixture.root.path().join("helper-result");
            std::fs::write(&helper, "#!/bin/sh\nif (printf 'WSP_DIFF_TTY\\n' > /dev/tty) 2>/dev/null; then printf 'tty\\n' > \"$WSP_DIFF_RESULT\"; printf 'external-diff-result\\n'; exit 0; fi\nprintf 'no-tty\\n' > \"$WSP_DIFF_RESULT\"\nprintf 'WSP_DIFF_NO_TTY\\n' >&2\nexit 1\n").unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
            if action == "st" {
                std::fs::write(&helper, "#!/bin/sh\nif (printf '' > /dev/tty) 2>/dev/null; then printf 'tty\n' > \"$WSP_DIFF_RESULT\"; else printf 'no-tty\n' > \"$WSP_DIFF_RESULT\"; fi\nprintf 'token\\000/\\000'\n").unwrap();
            }
            fixture.git(
                &fixture.clone,
                &[
                    "config",
                    if action == "st" {
                        "core.fsmonitor"
                    } else {
                        "diff.external"
                    },
                    helper.to_str().unwrap(),
                ],
            );
            let launcher = fixture.root.path().join("launch-diff");
            let status = fixture.root.path().join("cli-status");
            let flag = invocation
                .map(|mode| format!("--git-progress {mode}"))
                .unwrap_or_default();
            std::fs::write(&launcher, format!("#!/bin/sh\n\"$WSP_READ_BINARY\" {flag} {action}\nresult=$?\nprintf '%s' \"$result\" > \"$WSP_DIFF_STATUS\"\nexit \"$result\"\n")).unwrap();
            std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mut command = fixture.command("script");
            if cfg!(target_os = "macos") {
                command.args(["-q", "/dev/null"]).arg(&launcher);
            } else {
                command
                    .args(["-q", "-e", "-c"])
                    .arg(format!("'{}'", launcher.display()))
                    .arg("/dev/null");
            }
            let mut child = command
                .env("WSP_DIFF_RESULT", &result)
                .env("WSP_DIFF_STATUS", &status)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            // Keep the PTY input open until the helper finishes using its terminal.
            let _input = child.stdin.take().unwrap();
            let output = child.wait_with_output().unwrap();
            let transcript = String::from_utf8_lossy(&output.stdout);
            assert_eq!(
                std::fs::read_to_string(result).unwrap(),
                if terminal { "tty\n" } else { "no-tty\n" },
                "{global:?}/{repository:?}/{invocation:?}: {transcript}"
            );
            // The diff command reports individual errors as entries, so verify the
            // helper's result and returned content rather than only process status.
            assert_eq!(
                std::fs::read_to_string(status).unwrap(),
                "0",
                "{transcript}"
            );
            assert!(
                transcript.contains(if action == "st" {
                    "repo"
                } else if terminal {
                    "external-diff-result"
                } else {
                    "WSP_DIFF_NO_TTY"
                }),
                "helper outcome missing: {transcript}"
            );
        }
    }
}
