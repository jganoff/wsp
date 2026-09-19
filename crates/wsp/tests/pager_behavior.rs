use std::process::Command;

use assert_cmd::prelude::*;
use wsp_core::config::{Config, Paths};
use wsp_core::workspace::{self, Metadata};

fn output_pager(path: &std::path::Path) -> String {
    format!("cat > '{}'", path.display().to_string().replace('\\', "/"))
}

fn configure_command(command: &mut Command, root: &std::path::Path) {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    command
        .env("XDG_DATA_HOME", root)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("GIT_CONFIG_NOSYSTEM", "1");
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn workspace_with_change() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let paths = Paths::from_dirs(temp.path(), &temp.path().join("workspaces"));
    let workspace_dir = paths.workspaces_dir.join("pager-test");
    let repo_dir = workspace_dir.join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();

    let mut repos = std::collections::BTreeMap::new();
    repos.insert("test.local/owner/repo".to_string(), None);
    workspace::save_metadata(
        &workspace_dir,
        &Metadata {
            version: 0,
            name: "pager-test".into(),
            branch: "pager-test".into(),
            repos,
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        },
    )
    .unwrap();

    git(&repo_dir, &["init", "-q"]);
    git(&repo_dir, &["config", "user.email", "test@example.com"]);
    git(&repo_dir, &["config", "user.name", "Test"]);
    git(&repo_dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo_dir.join("file.txt"), "before\n").unwrap();
    git(&repo_dir, &["add", "file.txt"]);
    git(&repo_dir, &["commit", "-qm", "initial"]);
    std::fs::write(repo_dir.join("file.txt"), "before\nafter\n").unwrap();

    (temp, repo_dir)
}

#[test]
fn paginate_routes_a_complete_document_through_the_pager() {
    let dir = tempfile::tempdir().unwrap();
    let paged = dir.path().join("paged-output");

    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, dir.path());
    command
        .env("PAGER", output_pager(&paged))
        .args(["--paginate", "whatsnew"])
        .assert()
        .success()
        .stdout("");

    let output = std::fs::read_to_string(paged).unwrap();
    assert!(
        output.contains("What's new in wsp"),
        "pager did not receive the whatsnew document:\n{output}"
    );
}

#[test]
fn paginate_routes_finite_list_output_through_the_pager() {
    let dir = tempfile::tempdir().unwrap();
    let paged = dir.path().join("paged-output");
    let workspaces = dir.path().join("workspaces");
    std::fs::create_dir(&workspaces).unwrap();
    let isolated_workspace = workspaces.join("isolated-pager-test");
    std::fs::create_dir(&isolated_workspace).unwrap();
    workspace::save_metadata(
        &isolated_workspace,
        &Metadata {
            version: 0,
            name: "isolated-pager-test".into(),
            branch: "isolated-pager-test".into(),
            repos: std::collections::BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        },
    )
    .unwrap();
    let data_dir = dir.path().join("wsp");
    std::fs::create_dir(&data_dir).unwrap();
    Config {
        workspaces_dir: Some(workspaces.to_string_lossy().into_owned()),
        ..Default::default()
    }
    .save_to(&data_dir.join("config.yaml"))
    .unwrap();

    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, dir.path());
    command
        .env("PAGER", output_pager(&paged))
        .args(["--paginate", "ls"])
        .assert()
        .success()
        .stdout("");

    let output = std::fs::read_to_string(paged).unwrap();
    assert!(
        output.contains("isolated-pager-test"),
        "configured workspace listing was not paged:\n{output}"
    );
}

#[test]
fn explicit_help_documents_use_the_pager() {
    let dir = tempfile::tempdir().unwrap();
    let paged = dir.path().join("paged-output");

    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, dir.path());
    command
        .env("PAGER", output_pager(&paged))
        .args(["--paginate", "help", "config"])
        .assert()
        .success()
        .stdout("");

    let output = std::fs::read_to_string(paged).unwrap();
    assert!(
        output.contains("configuration keys and their effects"),
        "pager did not receive the help document:\n{output}"
    );
}

#[test]
fn forced_clap_help_uses_the_pager() {
    let dir = tempfile::tempdir().unwrap();
    let paged = dir.path().join("paged-output");

    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, dir.path());
    command
        .env("PAGER", output_pager(&paged))
        .args(["diff", "--paginate", "--help"])
        .assert()
        .success()
        .stdout("");

    assert!(
        std::fs::read_to_string(paged)
            .unwrap()
            .contains("Show git diff across workspace repos"),
        "forced clap help was not delivered to the pager"
    );
}

#[test]
fn non_tty_output_does_not_start_the_pager() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, dir.path());
    command
        .env("PAGER", "exit 23")
        .arg("whatsnew")
        .assert()
        .success()
        .stdout(predicates::str::contains("What's new in wsp"));
}

#[cfg(target_os = "linux")]
fn run_in_pty(
    args: &[&str],
    pager: &str,
    transcript: &std::path::Path,
    current_dir: Option<&std::path::Path>,
    data_home: Option<&std::path::Path>,
    git_config: Option<&std::path::Path>,
) -> std::process::ExitStatus {
    let binary = assert_cmd::cargo::cargo_bin("wsp");
    let invocation = format!("'{}' {}", binary.display(), args.join(" "));

    let mut command = Command::new("script");
    command
        .args(["-q", "-e", "-c", &invocation])
        .arg(transcript)
        .env("PAGER", pager);
    configure_pty_command(&mut command, current_dir, data_home, git_config);
    command.status().unwrap()
}

#[cfg(target_os = "macos")]
fn run_in_pty(
    args: &[&str],
    pager: &str,
    transcript: &std::path::Path,
    current_dir: Option<&std::path::Path>,
    data_home: Option<&std::path::Path>,
    git_config: Option<&std::path::Path>,
) -> std::process::ExitStatus {
    let binary = assert_cmd::cargo::cargo_bin("wsp");
    let mut command = Command::new("script");
    command
        .arg("-q")
        .arg(transcript)
        .arg(binary)
        .args(args)
        .env("PAGER", pager);
    configure_pty_command(&mut command, current_dir, data_home, git_config);
    command.status().unwrap()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn configure_pty_command(
    command: &mut Command,
    current_dir: Option<&std::path::Path>,
    data_home: Option<&std::path::Path>,
    git_config: Option<&std::path::Path>,
) {
    command.stdout(std::process::Stdio::null());
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    if let Some(data_home) = data_home {
        let home = data_home.join("home");
        std::fs::create_dir_all(&home).unwrap();
        command.env("XDG_DATA_HOME", data_home);
        command.env("HOME", &home);
        command.env("USERPROFILE", &home);
    }
    if let Some(git_config) = git_config {
        command.env("GIT_CONFIG_GLOBAL", git_config);
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env_remove("GIT_PAGER");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn interactive_output_starts_the_pager_automatically() {
    let dir = tempfile::tempdir().unwrap();
    let paged = dir.path().join("paged-output");
    let transcript = dir.path().join("transcript");

    let status = run_in_pty(
        &["whatsnew"],
        &output_pager(&paged),
        &transcript,
        None,
        Some(dir.path()),
        None,
    );

    assert!(status.success(), "script failed with {status}");
    let output = std::fs::read_to_string(paged).unwrap();
    assert!(
        output.contains("What's new in wsp"),
        "automatic pager did not receive the document:\n{output}"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn no_pager_overrides_an_interactive_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let paged = dir.path().join("paged-output");
    let transcript = dir.path().join("transcript");

    let status = run_in_pty(
        &["--no-pager", "whatsnew"],
        &output_pager(&paged),
        &transcript,
        None,
        Some(dir.path()),
        None,
    );

    assert!(status.success(), "script failed with {status}");
    assert!(!paged.exists(), "--no-pager still started the pager");
    let output = std::fs::read_to_string(transcript).unwrap();
    assert!(
        output.contains("What's new in wsp"),
        "direct output was not written to the terminal:\n{output}"
    );
}

#[test]
fn diff_and_log_use_their_command_specific_git_pagers() {
    let (temp, repo_dir) = workspace_with_change();
    let diff_output = temp.path().join("diff-output");
    let log_output = temp.path().join("log-output");
    let git_config = temp.path().join("gitconfig");
    std::fs::write(
        &git_config,
        format!(
            "[pager]\n\tdiff = {}\n\tlog = {}\n",
            output_pager(&diff_output),
            output_pager(&log_output)
        ),
    )
    .unwrap();

    let mut diff = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut diff, temp.path());
    diff.current_dir(&repo_dir)
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_PAGER")
        .env_remove("PAGER")
        .args(["--paginate", "diff", "--", "--name-only"])
        .assert()
        .success()
        .stdout("");
    assert!(
        std::fs::read_to_string(diff_output)
            .unwrap()
            .contains("file.txt"),
        "pager.diff did not receive the diff"
    );

    let mut log = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut log, temp.path());
    log.current_dir(repo_dir)
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_PAGER")
        .env_remove("PAGER")
        .args(["--paginate", "log", "--", "--all"])
        .assert()
        .success()
        .stdout("");
    assert!(
        std::fs::read_to_string(log_output)
            .unwrap()
            .contains("initial"),
        "pager.log did not receive the log"
    );
}

#[test]
fn included_global_git_pager_config_is_honored() {
    let (temp, repo_dir) = workspace_with_change();
    let paged = temp.path().join("paged-output");
    let included = temp.path().join("included-gitconfig");
    let git_config = temp.path().join("gitconfig");
    std::fs::write(
        &included,
        format!("[pager]\n\tdiff = {}\n", output_pager(&paged)),
    )
    .unwrap();
    std::fs::write(
        &git_config,
        format!(
            "[include]\n\tpath = {}\n",
            included.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();

    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, temp.path());
    command
        .current_dir(repo_dir)
        .env("GIT_CONFIG_GLOBAL", git_config)
        .env_remove("GIT_PAGER")
        .env_remove("PAGER")
        .args(["--paginate", "diff", "--", "--name-only"])
        .assert()
        .success()
        .stdout("");

    assert!(
        std::fs::read_to_string(paged).unwrap().contains("file.txt"),
        "pager.diff from an included global config was ignored"
    );
}

#[test]
fn forced_dry_run_sync_uses_the_pager() {
    let temp = tempfile::tempdir().unwrap();
    let workspace_dir = temp.path().join("workspaces").join("empty");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    workspace::save_metadata(
        &workspace_dir,
        &Metadata {
            version: 0,
            name: "empty".into(),
            branch: "empty".into(),
            repos: std::collections::BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        },
    )
    .unwrap();
    let paged = temp.path().join("paged-output");

    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, temp.path());
    command
        .current_dir(workspace_dir)
        .env("PAGER", output_pager(&paged))
        .args(["--paginate", "sync", "--dry-run"])
        .assert()
        .success()
        .stdout("");

    assert!(
        std::fs::read_to_string(paged)
            .unwrap()
            .contains("(dry run)"),
        "sync --dry-run did not use the forced pager"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn diff_and_log_page_automatically_on_a_terminal() {
    let (temp, repo_dir) = workspace_with_change();
    let diff_output = temp.path().join("diff-output");
    let log_output = temp.path().join("log-output");
    let diff_transcript = temp.path().join("diff-transcript");
    let log_transcript = temp.path().join("log-transcript");
    let git_config = temp.path().join("gitconfig");
    std::fs::write(
        &git_config,
        format!(
            "[pager]\n\tdiff = {}\n\tlog = {}\n",
            output_pager(&diff_output),
            output_pager(&log_output)
        ),
    )
    .unwrap();

    let diff_status = run_in_pty(
        &["diff", "--", "--name-only"],
        "exit 23",
        &diff_transcript,
        Some(&repo_dir),
        Some(temp.path()),
        Some(&git_config),
    );
    assert!(diff_status.success(), "script failed with {diff_status}");
    assert!(
        std::fs::read_to_string(diff_output)
            .unwrap()
            .contains("file.txt"),
        "automatic diff pager did not receive the diff"
    );

    let log_status = run_in_pty(
        &["log", "--", "--all"],
        "exit 23",
        &log_transcript,
        Some(&repo_dir),
        Some(temp.path()),
        Some(&git_config),
    );
    assert!(log_status.success(), "script failed with {log_status}");
    assert!(
        std::fs::read_to_string(log_output)
            .unwrap()
            .contains("initial"),
        "automatic log pager did not receive the log"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn command_config_false_and_empty_disable_automatic_paging() {
    for (name, setting) in [("false", "false"), ("empty", "")] {
        let (temp, repo_dir) = workspace_with_change();
        let transcript = temp.path().join(format!("{name}-transcript"));
        let git_config = temp.path().join("gitconfig");
        std::fs::write(&git_config, format!("[pager]\n\tdiff = {setting}\n")).unwrap();

        let status = run_in_pty(
            &["diff", "--", "--name-only"],
            "exit 23",
            &transcript,
            Some(&repo_dir),
            Some(temp.path()),
            Some(&git_config),
        );

        assert!(status.success(), "pager.diff={setting:?}: {status}");
        let output = std::fs::read_to_string(transcript).unwrap();
        assert!(
            output.contains("file.txt"),
            "pager.diff={setting:?} suppressed direct diff output:\n{output}"
        );
    }
}

#[test]
fn nonzero_numeric_command_config_uses_the_default_pager() {
    let (temp, repo_dir) = workspace_with_change();
    let paged = temp.path().join("paged-output");
    let git_config = temp.path().join("gitconfig");
    std::fs::write(&git_config, "[pager]\n\tdiff = 2\n").unwrap();

    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, temp.path());
    command
        .current_dir(repo_dir)
        .env("GIT_CONFIG_GLOBAL", git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PAGER", output_pager(&paged))
        .env_remove("GIT_PAGER")
        .args(["--paginate", "diff", "--", "--name-only"])
        .assert()
        .success()
        .stdout("");

    assert!(
        std::fs::read_to_string(paged).unwrap().contains("file.txt"),
        "numeric true did not select the default pager"
    );
}

#[test]
fn enclosing_repository_local_pager_config_is_ignored() {
    let (temp, repo_dir) = workspace_with_change();
    let paged = temp.path().join("paged-output");
    let empty_global = temp.path().join("empty-gitconfig");
    std::fs::write(&empty_global, "").unwrap();
    git(temp.path(), &["init", "-q"]);
    git(temp.path(), &["config", "pager.diff", "exit 23"]);

    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, temp.path());
    command
        .current_dir(repo_dir)
        .env("GIT_CONFIG_GLOBAL", empty_global)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PAGER", output_pager(&paged))
        .env_remove("GIT_PAGER")
        .args(["--paginate", "diff", "--", "--name-only"])
        .assert()
        .success()
        .stdout("");

    assert!(
        std::fs::read_to_string(paged).unwrap().contains("file.txt"),
        "an enclosing repository's local pager config controlled wsp"
    );
}

#[test]
fn json_never_starts_the_pager_even_when_pagination_is_forced() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("wsp").unwrap();
    configure_command(&mut command, dir.path());
    command
        .env("PAGER", "exit 23")
        .args(["--json", "--paginate", "whatsnew"])
        .assert()
        .success()
        .stdout(predicates::str::contains("What's new in wsp"));
}
