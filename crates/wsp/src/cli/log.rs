use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use super::completers;
use crate::output::print_gc_warning;
use wsp_core::config::Paths;
use wsp_core::gc;
use wsp_core::git;
use wsp_core::output::{LogCommit, LogOutput, Output, RepoLogEntry};
use wsp_core::workspace;

pub fn cmd() -> Command {
    Command::new("log")
        .add(crate::shellnav::ShellNav::none())
        .about("Show commits ahead of upstream per workspace repo [read-only]")
        .long_about(
            "Show commits ahead of upstream per workspace repo [read-only].\n\n\
             Lists unpushed commits on the workspace branch for each repo. Use --oneline \
             for a flat chronological view across all repos. Extra arguments after `--` are \
             forwarded to git log.",
        )
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("oneline")
                .long("oneline")
                .action(ArgAction::SetTrue)
                .help("Flat chronological view across all repos"),
        )
        .arg(
            Arg::new("args")
                .num_args(1..)
                .last(true)
                .allow_hyphen_values(true),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let ws_dir: PathBuf = if let Some(name) = matches.get_one::<String>("workspace") {
        workspace::dir(&paths.workspaces_dir, name)
    } else {
        let cwd = crate::shellcd::invocation_dir()?;
        workspace::detect(&cwd)?
    };

    if let Some(warning) = gc::check_workspace(&ws_dir, /* read_only */ true)? {
        print_gc_warning(&warning);
    }

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let extra_args: Vec<&str> = matches
        .get_many::<String>("args")
        .map(|vals| vals.map(|s| s.as_str()).collect())
        .unwrap_or_default();

    let is_json = matches.get_flag("json");
    let is_oneline = matches.get_flag("oneline");
    let use_color = !is_json && !is_oneline && std::io::stdout().is_terminal();

    let mut repos = Vec::new();
    for identity in meta.repos.keys() {
        let dir_name = match meta.dir_name(identity) {
            Ok(d) => d,
            Err(e) => {
                repos.push(RepoLogEntry {
                    identity: identity.clone(),
                    shortname: identity.rsplit('/').next().unwrap_or(identity).to_string(),
                    path: String::new(),
                    commits: vec![],
                    raw: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        let repo_dir = ws_dir.join(&dir_name);

        if !extra_args.is_empty() {
            // Pass-through mode: run git log with user-supplied args verbatim
            let mut args = vec!["log"];
            if use_color {
                args.push("--color=always");
            }
            args.extend(&extra_args);

            match git::run(Some(&repo_dir), &args) {
                Ok(output) => {
                    repos.push(RepoLogEntry {
                        identity: identity.clone(),
                        shortname: dir_name,
                        path: repo_dir.to_string_lossy().to_string(),
                        commits: vec![],
                        raw: Some(output),
                        error: None,
                    });
                }
                Err(e) => {
                    repos.push(RepoLogEntry {
                        identity: identity.clone(),
                        shortname: dir_name,
                        path: repo_dir.to_string_lossy().to_string(),
                        commits: vec![],
                        raw: None,
                        error: Some(e.to_string()),
                    });
                }
            }
        } else {
            // Structured mode: parse commits from upstream..HEAD
            match resolve_log_range(&repo_dir) {
                Some(range) => match fetch_commits(&repo_dir, &range) {
                    Ok(commits) => {
                        repos.push(RepoLogEntry {
                            identity: identity.clone(),
                            shortname: dir_name,
                            path: repo_dir.to_string_lossy().to_string(),
                            commits,
                            raw: None,
                            error: None,
                        });
                    }
                    Err(e) => {
                        repos.push(RepoLogEntry {
                            identity: identity.clone(),
                            shortname: dir_name,
                            path: repo_dir.to_string_lossy().to_string(),
                            commits: vec![],
                            raw: None,
                            error: Some(e.to_string()),
                        });
                    }
                },
                None => {
                    // No upstream — skip this repo silently (no range to compare)
                    repos.push(RepoLogEntry {
                        identity: identity.clone(),
                        shortname: dir_name,
                        path: repo_dir.to_string_lossy().to_string(),
                        commits: vec![],
                        raw: None,
                        error: None,
                    });
                }
            }
        }
    }

    Ok(Output::Log(LogOutput {
        workspace: meta.name,
        branch: meta.branch,
        workspace_dir: ws_dir,
        repos,
        oneline: is_oneline,
    }))
}

/// Resolve the log range for the current branch relative to its upstream.
/// Returns None if there's no upstream to compare against.
fn resolve_log_range(repo_dir: &Path) -> Option<String> {
    match git::resolve_upstream_ref(repo_dir) {
        git::UpstreamRef::Tracking => Some("@{upstream}..HEAD".to_string()),
        git::UpstreamRef::DefaultBranch(b) => Some(format!("origin/{}..HEAD", b)),
        git::UpstreamRef::Head => None,
    }
}

/// Run `git log --format=...` and parse each line into a LogCommit.
/// Uses NUL byte (%x00) as field separator to handle subjects with spaces
/// or empty subjects without silent data loss.
fn fetch_commits(repo_dir: &Path, range: &str) -> Result<Vec<LogCommit>> {
    let output = git::run(Some(repo_dir), &["log", "--format=%H%x00%ct%x00%s", range])?;
    if output.is_empty() {
        return Ok(vec![]);
    }

    let mut commits = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.splitn(3, '\0').collect();
        if parts.len() < 3 {
            continue;
        }
        let timestamp = parts[1].parse::<i64>().unwrap_or(0);
        let authored_at = chrono::DateTime::from_timestamp(timestamp, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        commits.push(LogCommit {
            hash: parts[0].to_string(),
            authored_at,
            timestamp,
            subject: parts[2].to_string(),
        });
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    /// Creates a temp git repo with a configurable number of commits.
    /// Returns the repo path and TempDir handle.
    fn setup_repo(commit_count: usize) -> (PathBuf, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        for args in &[
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }

        for i in 0..commit_count {
            let file = format!("file{}.txt", i);
            std::fs::write(dir.join(&file), format!("content {}", i)).unwrap();
            let out = StdCommand::new("git")
                .args(["add", &file])
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success());
            let msg = format!("commit {}", i);
            let out = StdCommand::new("git")
                .args(["commit", "-m", &msg])
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "commit: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        (dir, tmp)
    }

    #[test]
    fn test_fetch_commits_parses() {
        let (dir, _tmp) = setup_repo(3);

        // Range: HEAD~2..HEAD should give 2 commits
        let commits = fetch_commits(&dir, "HEAD~2..HEAD").unwrap();
        assert_eq!(commits.len(), 2, "expected 2 commits");

        // Verify structure
        for c in &commits {
            assert_eq!(c.hash.len(), 40, "hash should be 40 chars: {}", c.hash);
            assert!(c.timestamp > 0, "timestamp should be positive");
            assert!(!c.subject.is_empty(), "subject should not be empty");
        }

        // Most recent commit first (git log default order)
        assert_eq!(commits[0].subject, "commit 2");
        assert_eq!(commits[1].subject, "commit 1");
    }

    #[test]
    fn test_fetch_commits_empty_range() {
        let (dir, _tmp) = setup_repo(1);
        // HEAD..HEAD is an empty range
        let commits = fetch_commits(&dir, "HEAD..HEAD").unwrap();
        assert!(commits.is_empty());
    }

    #[test]
    fn test_resolve_log_range_with_default_branch() {
        let source_tmp = tempfile::tempdir().unwrap();
        let source = source_tmp.path().to_path_buf();

        // Create source repo
        for args in &[
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(&source)
                .output()
                .unwrap();
            assert!(out.status.success());
        }

        // Clone it
        let clone_tmp = tempfile::tempdir().unwrap();
        let clone_dir = clone_tmp.path().join("repo");
        let out = StdCommand::new("git")
            .args([
                "clone",
                source.to_str().unwrap(),
                clone_dir.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(out.status.success());

        // Set origin/HEAD
        let out = StdCommand::new("git")
            .args(["remote", "set-head", "origin", "main"])
            .current_dir(&clone_dir)
            .output()
            .unwrap();
        assert!(out.status.success());

        // Create a new branch (no tracking)
        let out = StdCommand::new("git")
            .args(["checkout", "-b", "feature", "--no-track", "origin/main"])
            .current_dir(&clone_dir)
            .output()
            .unwrap();
        assert!(out.status.success());

        let range = resolve_log_range(&clone_dir);
        assert_eq!(range, Some("origin/main..HEAD".to_string()));
    }

    #[test]
    fn test_resolve_log_range_no_remote_falls_back_to_default_branch() {
        // Repo with no remote — default_branch() falls back to symbolic-ref HEAD,
        // so resolve_log_range returns DefaultBranch("main") → "origin/main..HEAD".
        // In practice this range would fail at git log time since origin doesn't exist,
        // but workspace repos always have an origin.
        let (dir, _tmp) = setup_repo(1);
        let range = resolve_log_range(&dir);
        assert_eq!(range, Some("origin/main..HEAD".to_string()));
    }

    #[test]
    fn test_resolve_log_range_with_tracking() {
        let source_tmp = tempfile::tempdir().unwrap();
        let source = source_tmp.path().to_path_buf();

        for args in &[
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(&source)
                .output()
                .unwrap();
            assert!(out.status.success());
        }

        let clone_tmp = tempfile::tempdir().unwrap();
        let clone_dir = clone_tmp.path().join("repo");
        let out = StdCommand::new("git")
            .args([
                "clone",
                source.to_str().unwrap(),
                clone_dir.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(out.status.success());

        // main tracks origin/main by default
        let range = resolve_log_range(&clone_dir);
        assert_eq!(range, Some("@{upstream}..HEAD".to_string()));
    }
}
