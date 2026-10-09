//! Git hooks exercise progress-policy scope beyond sync's fetch phase.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use wsp_core::config::{Config, Paths, ProgressConfig, RepoEntry};
use wsp_core::git_policy::Mode;
use wsp_core::giturl::Parsed;
use wsp_core::workspace::{self, Metadata};

const IDENTITY: &str = "example.test/team/repo";
const URL: &str = "https://example.test/team/repo.git";
const WORKSPACE: &str = "feature";

#[derive(Clone, Copy, Debug)]
enum Selection {
    RepositoryNative,
    InvocationNative,
    InvocationParallel,
}

impl Selection {
    fn expects_terminal(self) -> bool {
        !matches!(self, Self::InvocationParallel)
    }

    fn argument(self) -> &'static str {
        match self {
            Self::RepositoryNative => "",
            Self::InvocationNative => "--git-progress native",
            Self::InvocationParallel => "--git-progress parallel",
        }
    }
}

struct Fixture {
    root: tempfile::TempDir,
    git_config: PathBuf,
    data: PathBuf,
    clone: PathBuf,
    marker: PathBuf,
    upstream_head: String,
    local_head: String,
}

impl Fixture {
    fn git_output(&self, dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", &self.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .output()
            .unwrap()
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.git_output(dir, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn commit(&self, dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
        self.git(dir, &["add", name]);
        self.git(dir, &["commit", "--quiet", "-m", name]);
    }

    fn new(selection: Selection, conflict: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let git_config = root.path().join("gitconfig");
        std::fs::write(
            &git_config,
            "[user]\n name = Fixture\n email = fixture@example.test\n[commit]\n gpgsign = false\n",
        )
        .unwrap();
        let data = root.path().join("data");
        let paths = Paths::from_dirs(&data.join("wsp"), &root.path().join("workspaces"));
        let workspace_dir = paths.workspaces_dir.join(WORKSPACE);
        let clone = workspace_dir.join("repo");
        let marker = root.path().join("hook-result");
        let mut fixture = Self {
            root,
            git_config,
            data,
            clone,
            marker,
            upstream_head: String::new(),
            local_head: String::new(),
        };
        let source = fixture.root.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&workspace_dir).unwrap();
        fixture.git(&source, &["init", "--quiet", "--initial-branch=main"]);
        fixture.commit(&source, "shared.txt", "initial\n");
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
            &[
                "checkout",
                "--quiet",
                "-b",
                WORKSPACE,
                "--no-track",
                "origin/main",
            ],
        );
        fixture.commit(
            &fixture.clone,
            if conflict { "shared.txt" } else { "local.txt" },
            "local\n",
        );
        fixture.local_head = fixture.git(&fixture.clone, &["rev-parse", "HEAD"]);
        fixture.commit(
            &source,
            if conflict {
                "shared.txt"
            } else {
                "upstream.txt"
            },
            "upstream\n",
        );
        fixture.upstream_head = fixture.git(&source, &["rev-parse", "HEAD"]);
        let mirror_dir = wsp_core::mirror::dir(
            &paths.mirrors_dir,
            &Parsed::from_identity(IDENTITY).unwrap(),
        );
        std::fs::create_dir_all(mirror_dir.parent().unwrap()).unwrap();
        fixture.git(
            fixture.root.path(),
            &[
                "clone",
                "--quiet",
                "--mirror",
                source.to_str().unwrap(),
                mirror_dir.to_str().unwrap(),
            ],
        );
        for repo in [&fixture.clone, &mirror_dir] {
            fixture.git(repo, &["remote", "set-url", "origin", URL]);
        }
        let global = std::fs::read_to_string(&fixture.git_config).unwrap();
        std::fs::write(
            &fixture.git_config,
            format!(
                "{global}[url \"file://{}\"]\n insteadOf = {URL}\n",
                source.display()
            ),
        )
        .unwrap();
        let repository_mode = if matches!(selection, Selection::InvocationNative) {
            Mode::Parallel
        } else {
            Mode::Native
        };
        Config {
            workspaces_dir: Some(paths.workspaces_dir.display().to_string()),
            hints: Some(false),
            progress: Some(ProgressConfig {
                mode: Some(Mode::Parallel),
                repos: BTreeMap::from([(IDENTITY.into(), repository_mode)]),
            }),
            repos: BTreeMap::from([(
                IDENTITY.into(),
                RepoEntry {
                    url: URL.into(),
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
                name: WORKSPACE.into(),
                branch: WORKSPACE.into(),
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

    fn terminal_hook(&self, name: &str) {
        let hook = self.clone.join(".git/hooks").join(name);
        // The hook's exit status affects the real Git operation. Merely finding
        // the hook file or checking a terminal flag would not verify dispatch.
        std::fs::write(&hook, "#!/bin/sh\nif (printf 'WSP_SYNC_HOOK_TTY\\n' > /dev/tty) 2>/dev/null; then\n printf 'tty\\n' > \"$WSP_SYNC_HOOK_RESULT\"\n exit 0\nfi\nprintf 'no-tty\\n' > \"$WSP_SYNC_HOOK_RESULT\"\nprintf 'WSP_SYNC_HOOK_NO_TTY\\n' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn sync(&self, selection: Selection, strategy: &str) -> (i32, String) {
        let launcher = self.root.path().join("sync-in-terminal");
        let status = self.root.path().join("sync-status");
        // Capture the CLI's own status because BSD script does not propagate it.
        // All interpolated arguments below are fixed test enum/strategy values.
        std::fs::write(&launcher, format!("#!/bin/sh\n\"$WSP_SYNC_BINARY\" sync feature --strategy {strategy} {}\nresult=$?\nprintf '%s' \"$result\" > \"$WSP_SYNC_STATUS\"\nexit \"$result\"\n", selection.argument())).unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut command = Command::new("script");
        if cfg!(target_os = "macos") {
            command.args(["-q", "/dev/null"]).arg(&launcher);
        } else {
            command
                .args(["-q", "-e", "-c"])
                .arg(format!("'{}'", launcher.display()))
                .arg("/dev/null");
        }
        let output = command
            .current_dir(self.root.path())
            .env("HOME", self.root.path())
            .env("XDG_DATA_HOME", &self.data)
            .env("GIT_CONFIG_GLOBAL", &self.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_EDITOR", "true")
            .env(
                "WSP_SYNC_BINARY",
                std::env::var_os("WSP_PROGRESS_BASELINE")
                    .unwrap_or_else(|| env!("CARGO_BIN_EXE_wsp").into()),
            )
            .env("WSP_SYNC_STATUS", &status)
            .env("WSP_SYNC_HOOK_RESULT", &self.marker)
            .env_remove("WSP_PWD")
            .env_remove("WSP_SHELL")
            .env_remove("WSP_CD_FILE")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let code = std::fs::read_to_string(&status)
            .unwrap_or_else(|error| panic!("missing CLI exit status: {error}; terminal: {text}"));
        (code.parse().unwrap(), text)
    }

    fn verify(&self, selection: Selection, status: i32, terminal: &str) {
        let observed = std::fs::read_to_string(&self.marker).unwrap_or_else(|error| {
            panic!("hook must execute for {selection:?}: {error}; terminal: {terminal}")
        });
        if selection.expects_terminal() {
            assert_eq!(observed, "tty\n", "{selection:?}: {terminal}");
            assert_eq!(status, 0, "native hook failed: {terminal}");
            assert!(
                terminal.contains("WSP_SYNC_HOOK_TTY"),
                "native hook terminal output missing: {terminal}"
            );
            self.git(
                &self.clone,
                &["merge-base", "--is-ancestor", &self.upstream_head, "HEAD"],
            );
        } else {
            assert_eq!(observed, "no-tty\n", "{selection:?}: {terminal}");
            assert_ne!(
                status, 0,
                "parallel mode accepted a failing terminal hook: {terminal}"
            );
        }
    }
}

#[test]
fn sync_progress_policy_reaches_rebase_and_merge_hooks_after_fetch() {
    for strategy in ["rebase", "merge"] {
        for selection in [
            Selection::RepositoryNative,
            Selection::InvocationNative,
            Selection::InvocationParallel,
        ] {
            let fixture = Fixture::new(selection, false);
            fixture.terminal_hook(if strategy == "rebase" {
                "pre-rebase"
            } else {
                "pre-merge-commit"
            });
            let (status, terminal) = fixture.sync(selection, strategy);
            fixture.verify(selection, status, &terminal);
            assert_eq!(
                std::fs::read_to_string(fixture.clone.join("local.txt")).unwrap(),
                "local\n"
            );
            if selection.expects_terminal() {
                assert_eq!(
                    std::fs::read_to_string(fixture.clone.join("upstream.txt")).unwrap(),
                    "upstream\n"
                );
            } else {
                assert_eq!(
                    fixture.git(&fixture.clone, &["rev-parse", "HEAD"]),
                    fixture.local_head
                );
            }
        }
    }
}

#[test]
fn sync_progress_policy_reaches_rebase_and_merge_continuation_hooks() {
    for strategy in ["rebase", "merge"] {
        for selection in [
            Selection::RepositoryNative,
            Selection::InvocationNative,
            Selection::InvocationParallel,
        ] {
            let fixture = Fixture::new(selection, true);
            fixture.git(&fixture.clone, &["fetch", "--quiet", "origin"]);
            let output = fixture.git_output(&fixture.clone, &[strategy, "origin/main"]);
            assert!(
                !output.status.success(),
                "fixture must stop at a real conflict"
            );
            let state = fixture.clone.join(if strategy == "rebase" {
                ".git/rebase-merge"
            } else {
                ".git/MERGE_HEAD"
            });
            assert!(state.exists(), "{strategy} conflict must persist");
            std::fs::write(fixture.clone.join("shared.txt"), "resolved\n").unwrap();
            fixture.git(&fixture.clone, &["add", "shared.txt"]);
            fixture.terminal_hook("prepare-commit-msg");
            let (status, terminal) = fixture.sync(selection, strategy);
            fixture.verify(selection, status, &terminal);
            assert_eq!(
                state.exists(),
                !selection.expects_terminal(),
                "continuation state incorrect for {selection:?}: {terminal}"
            );
            assert_eq!(
                std::fs::read_to_string(fixture.clone.join("shared.txt")).unwrap(),
                "resolved\n"
            );
        }
    }
}
