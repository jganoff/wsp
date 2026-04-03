use std::path::PathBuf;

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::output::print_gc_warning;
use wsp_core::config::{self, Paths};
use wsp_core::gc;
use wsp_core::git;
use wsp_core::output::{Output, RepoStatusEntry, StatusOutput};
use wsp_core::workspace;

use super::completers;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::build_cli;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::process::Command as StdCommand;
    use wsp_core::config::Paths;
    use wsp_core::workspace;

    fn dummy_paths() -> Paths {
        Paths {
            config_path: PathBuf::from("/nonexistent/config.yaml"),
            mirrors_dir: PathBuf::from("/nonexistent/mirrors"),
            gc_dir: PathBuf::from("/nonexistent/gc"),
            templates_dir: PathBuf::from("/nonexistent/templates"),
            workspaces_dir: PathBuf::from("/nonexistent/workspaces"),
        }
    }

    /// Build Paths rooted under `tmp`. Callers must create the directories they need.
    fn test_paths(tmp: &std::path::Path) -> Paths {
        Paths {
            config_path: tmp.join("config.yaml"),
            mirrors_dir: tmp.join("mirrors"),
            gc_dir: tmp.join("gc"),
            templates_dir: tmp.join("templates"),
            workspaces_dir: tmp.join("workspaces"),
        }
    }

    /// Build a Metadata with sensible defaults. Repos/dirs are provided by the caller.
    fn test_metadata(
        name: &str,
        branch: &str,
        repos: BTreeMap<String, Option<workspace::WorkspaceRepoRef>>,
        dirs: BTreeMap<String, String>,
    ) -> workspace::Metadata {
        workspace::Metadata {
            version: 0,
            name: name.into(),
            branch: branch.into(),
            repos,
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs,
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        }
    }

    /// Initialise a bare-minimum local git repo at `dir` on branch `main`.
    /// No remote / upstream — sufficient for status checks that only need
    /// a valid HEAD and working tree.
    fn init_git_repo_at(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).unwrap();
        for args in &[
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// Create a workspace directory at `ws_dir` containing a single git repo at
    /// `ws_dir/myrepo/` registered as `github.com/testorg/myrepo`. The workspace
    /// branch is set to `branch`.
    fn setup_single_repo_workspace(ws_dir: &std::path::Path, branch: &str) {
        init_git_repo_at(&ws_dir.join("myrepo"));

        let mut repos = BTreeMap::new();
        repos.insert("github.com/testorg/myrepo".to_string(), None);
        let mut dirs = BTreeMap::new();
        dirs.insert(
            "github.com/testorg/myrepo".to_string(),
            "myrepo".to_string(),
        );

        let meta = test_metadata("test-ws", branch, repos, dirs);
        workspace::save_metadata(ws_dir, &meta).unwrap();
    }

    #[test]
    fn run_with_root_matches_does_not_panic() {
        // When `ws` is run with no subcommand inside a workspace, dispatch
        // passes root-level ArgMatches (which lack a "workspace" arg) to
        // status::run. This must not panic — it should gracefully fall
        // through to workspace detection via cwd.
        let matches = build_cli().get_matches_from(["wsp"]);

        // The only thing we're testing is that this doesn't panic.
        // The result depends on whether tests run inside a workspace.
        let _ = run(&matches, &dummy_paths());
    }

    #[test]
    fn status_empty_workspace_succeeds() {
        // An empty workspace (no repos) should run without error and return
        // a StatusOutput with an empty repos list.
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let ws_dir = paths.workspaces_dir.join("test-ws");
        std::fs::create_dir_all(&ws_dir).unwrap();

        let meta = test_metadata("test-ws", "main", BTreeMap::new(), BTreeMap::new());
        workspace::save_metadata(&ws_dir, &meta).unwrap();

        let m = cmd().get_matches_from(["st", "test-ws"]);
        let out = run(&m, &paths).unwrap();

        match out {
            Output::Status(s) => {
                assert_eq!(s.workspace, "test-ws");
                assert_eq!(s.branch, "main");
                assert!(s.repos.is_empty(), "empty workspace should have no repos");
            }
            _ => panic!("expected Status output"),
        }
    }

    #[test]
    fn status_clean_repo_shows_no_changes() {
        // A workspace with a clean repo should report zero changed files and no
        // expected_branch mismatch when the repo is on the workspace branch.
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let ws_dir = paths.workspaces_dir.join("test-ws");
        std::fs::create_dir_all(&ws_dir).unwrap();

        // Repo is initialised on "main"; workspace branch is also "main" → no mismatch.
        setup_single_repo_workspace(&ws_dir, "main");

        let m = cmd().get_matches_from(["st", "test-ws"]);
        let out = run(&m, &paths).unwrap();

        match out {
            Output::Status(s) => {
                assert_eq!(s.repos.len(), 1);
                let repo = &s.repos[0];
                assert_eq!(repo.changed, 0, "clean repo should have no changed files");
                assert!(repo.error.is_none(), "clean repo should have no error");
                assert_eq!(
                    repo.expected_branch, None,
                    "repo on the workspace branch should not set expected_branch"
                );
            }
            _ => panic!("expected Status output"),
        }
    }

    #[test]
    fn status_dirty_repo_shows_changed_count() {
        // After writing an untracked file into a repo, `changed` should be > 0
        // and the file should appear in the per-repo files list.
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let ws_dir = paths.workspaces_dir.join("test-ws");
        std::fs::create_dir_all(&ws_dir).unwrap();

        setup_single_repo_workspace(&ws_dir, "main");

        // Introduce an untracked file to make the working tree dirty.
        std::fs::write(ws_dir.join("myrepo").join("dirty.txt"), "uncommitted").unwrap();

        let m = cmd().get_matches_from(["st", "test-ws"]);
        let out = run(&m, &paths).unwrap();

        match out {
            Output::Status(s) => {
                assert_eq!(s.repos.len(), 1);
                let repo = &s.repos[0];
                assert!(
                    repo.changed > 0,
                    "dirty repo should report at least one changed file"
                );
                assert!(
                    repo.files.iter().any(|f| f.contains("dirty.txt")),
                    "dirty.txt should appear in the file list; got: {:?}",
                    repo.files
                );
            }
            _ => panic!("expected Status output"),
        }
    }

    #[test]
    fn status_wrong_branch_sets_expected_branch() {
        // When a repo's HEAD is on a different branch than the workspace branch,
        // `expected_branch` should be populated with the workspace branch name.
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let ws_dir = paths.workspaces_dir.join("test-ws");
        std::fs::create_dir_all(&ws_dir).unwrap();

        // Workspace branch is "test/test-ws"; repo will be switched to "feature/my-thing".
        setup_single_repo_workspace(&ws_dir, "test/test-ws");

        // Switch the repo to a branch that is NOT the workspace branch.
        let repo_dir = ws_dir.join("myrepo");
        let out = StdCommand::new("git")
            .args(["checkout", "-b", "feature/my-thing"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "checkout: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let m = cmd().get_matches_from(["st", "test-ws"]);
        let out = run(&m, &paths).unwrap();

        match out {
            Output::Status(s) => {
                assert_eq!(s.repos.len(), 1);
                let repo = &s.repos[0];
                assert_eq!(repo.branch, "feature/my-thing");
                assert_eq!(
                    repo.expected_branch,
                    Some("test/test-ws".to_string()),
                    "wrong-branch repo should expose expected_branch set to the workspace branch"
                );
            }
            _ => panic!("expected Status output"),
        }
    }
}

pub fn cmd() -> Command {
    Command::new("st")
        .visible_alias("status")
        .about("Git status across workspace repos [read-only]")
        .long_about(
            "Git status across workspace repos [read-only].\n\n\
             Shows each repo's branch, commits ahead/behind upstream, and number of \
             changed files. Detects wrong-branch checkouts and warns when HEAD differs \
             from the workspace branch. Also reports unexpected files in the workspace root.\n\n\
             Paths listed in `.wspignore` (at workspace root) or the global \
             `~/.local/share/wsp/wspignore` are suppressed from root checks.",
        )
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Show per-repo file lists")
                .action(clap::ArgAction::SetTrue),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let ws_dir: PathBuf =
        if let Some(name) = matches.try_get_one::<String>("workspace").ok().flatten() {
            workspace::dir(&paths.workspaces_dir, name)
        } else {
            let cwd = std::env::current_dir()?;
            workspace::detect(&cwd)?
        };

    if let Some(warning) = gc::check_workspace(&ws_dir, /* read_only */ true)? {
        print_gc_warning(&warning);
    }

    let verbose = matches
        .try_get_one::<bool>("verbose")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false);

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let mut repos = Vec::new();

    for identity in meta.repos.keys() {
        let dir_name = match meta.dir_name(identity) {
            Ok(d) => d,
            Err(e) => {
                repos.push(RepoStatusEntry {
                    identity: identity.clone(),
                    shortname: identity.rsplit('/').next().unwrap_or(identity).to_string(),
                    path: String::new(),
                    branch: String::new(),
                    ahead: 0,
                    behind: 0,
                    changed: 0,
                    has_upstream: false,
                    role: "active".into(),
                    files: vec![],
                    error: Some(e.to_string()),
                    expected_branch: None,
                    pr: None,
                });
                continue;
            }
        };

        let repo_dir = ws_dir.join(&dir_name);

        let branch = git::branch_current(&repo_dir).unwrap_or_else(|_| "?".to_string());

        // Detect wrong-branch: HEAD differs from workspace branch
        let expected_branch = if branch != meta.branch && branch != "?" {
            Some(meta.branch.clone())
        } else {
            None
        };

        let upstream = git::resolve_upstream_ref(&repo_dir);
        let has_upstream = matches!(upstream, git::UpstreamRef::Tracking);
        let ahead = git::ahead_count_from(&repo_dir, &upstream).unwrap_or(0);
        let behind = git::behind_count_from(&repo_dir, &upstream).unwrap_or(0);
        let files = git::changed_files(&repo_dir).unwrap_or_default();
        let changed = files.len() as u32;
        repos.push(RepoStatusEntry {
            identity: identity.clone(),
            shortname: dir_name.clone(),
            path: repo_dir.to_string_lossy().to_string(),
            branch,
            ahead,
            behind,
            changed,
            has_upstream,
            role: "active".into(),
            files,
            error: None,
            expected_branch,
            pr: None, // filled in below when pr.source is set
        });
    }

    // Fetch PR data in parallel when `pr.source = gh` is set in config.
    let cfg = config::Config::load_from(&paths.config_path).unwrap_or_default();
    if cfg.pr_source.as_deref().is_some_and(|s| s != "false") {
        let inputs: Vec<(String, String)> = repos
            .iter()
            .map(|r| (r.identity.clone(), meta.branch.clone()))
            .collect();
        let pr_results = crate::pr::fetch_parallel(&inputs);
        for (repo, pr) in repos.iter_mut().zip(pr_results) {
            repo.pr = pr;
        }
    }

    let ignore = workspace::load_wspignore(paths.data_dir(), &ws_dir);
    let root = match workspace::check_root_content(&ws_dir, &meta) {
        Ok(items) => {
            let filtered = workspace::filter_ignored(&items, &ignore);
            filtered.iter().map(|p| p.to_string()).collect()
        }
        Err(e) => {
            eprintln!("  warning: root content check failed: {}", e);
            vec![]
        }
    };

    Ok(Output::Status(StatusOutput {
        workspace: meta.name,
        branch: meta.branch,
        workspace_dir: ws_dir,
        description: meta.description,
        created: meta.created,
        repos,
        root,
        verbose,
    }))
}
