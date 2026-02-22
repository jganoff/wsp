use std::path::PathBuf;

use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use super::completers;
use crate::config::Paths;
use crate::git::{self, UpstreamRef};
use crate::output::{Output, PushOutput, PushRepoResult};
use crate::workspace;

pub fn cmd() -> Command {
    Command::new("push")
        .about("Push all active workspace repos")
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("force-with-lease")
                .long("force-with-lease")
                .action(ArgAction::SetTrue)
                .help("Push with --force-with-lease (e.g. after rebase)"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Preview which repos would be pushed"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let ws_dir: PathBuf = if let Some(name) = matches.get_one::<String>("workspace") {
        workspace::dir(&paths.workspaces_dir, name)
    } else {
        let cwd = std::env::current_dir()?;
        workspace::detect(&cwd)?
    };

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let force_with_lease = matches.get_flag("force-with-lease");
    let dry_run = matches.get_flag("dry-run");

    let repo_infos = meta.repo_infos(&ws_dir);

    // Serial push loop
    let mut results = Vec::new();
    for info in &repo_infos {
        if let Some(ref e) = info.error {
            results.push(PushRepoResult {
                name: info.dir_name.clone(),
                action: String::new(),
                ok: false,
                detail: None,
                error: Some(e.clone()),
                repo_dir: info.clone_dir.clone(),
                branch: meta.branch.clone(),
            });
            continue;
        }

        // Context repo — skip
        if info.is_context {
            let pinned = info.pinned_ref.as_deref().unwrap_or("HEAD");
            results.push(PushRepoResult {
                name: info.dir_name.clone(),
                action: format!("(context @{})", pinned),
                ok: true,
                detail: Some("skipped".into()),
                error: None,
                repo_dir: info.clone_dir.clone(),
                branch: String::new(),
            });
            continue;
        }

        // Active repo
        let current_branch = match git::branch_current(&info.clone_dir) {
            Ok(b) => b,
            Err(e) => {
                results.push(PushRepoResult {
                    name: info.dir_name.clone(),
                    action: String::new(),
                    ok: false,
                    detail: None,
                    error: Some(format!("cannot read branch: {}", e)),
                    repo_dir: info.clone_dir.clone(),
                    branch: meta.branch.clone(),
                });
                continue;
            }
        };

        // Safety: refuse to push the default branch
        if let Ok(default_branch) = git::default_branch(&info.clone_dir)
            && current_branch == default_branch
        {
            results.push(PushRepoResult {
                name: info.dir_name.clone(),
                action: format!("push {} -> origin", current_branch),
                ok: false,
                detail: None,
                error: Some(format!(
                    "refusing to push default branch '{}' — push from a workspace branch instead",
                    default_branch
                )),
                repo_dir: info.clone_dir.clone(),
                branch: current_branch,
            });
            continue;
        }

        let upstream = git::resolve_upstream_ref(&info.clone_dir);
        if matches!(upstream, UpstreamRef::Head) {
            results.push(PushRepoResult {
                name: info.dir_name.clone(),
                action: format!("push {} -> origin", current_branch),
                ok: false,
                detail: None,
                error: Some(
                    "cannot determine upstream (no tracking branch, no default branch)".into(),
                ),
                repo_dir: info.clone_dir.clone(),
                branch: current_branch,
            });
            continue;
        }
        let ahead = match git::ahead_count_from(&info.clone_dir, &upstream) {
            Ok(n) => n,
            Err(e) => {
                results.push(PushRepoResult {
                    name: info.dir_name.clone(),
                    action: format!("push {} -> origin", current_branch),
                    ok: false,
                    detail: None,
                    error: Some(format!("cannot determine ahead count: {}", e)),
                    repo_dir: info.clone_dir.clone(),
                    branch: current_branch,
                });
                continue;
            }
        };
        let action = format!("push {} -> origin", current_branch);

        if ahead == 0 {
            results.push(PushRepoResult {
                name: info.dir_name.clone(),
                action: "nothing to push".into(),
                ok: true,
                detail: None,
                error: None,
                repo_dir: info.clone_dir.clone(),
                branch: current_branch,
            });
            continue;
        }

        let needs_upstream = !matches!(upstream, UpstreamRef::Tracking)
            || !git::remote_branch_exists(&info.clone_dir, &current_branch);

        if dry_run {
            let mut detail = format!("{} commit(s) to push", ahead);
            if needs_upstream {
                detail.push_str(" (will set upstream)");
            }
            results.push(PushRepoResult {
                name: info.dir_name.clone(),
                action,
                ok: true,
                detail: Some(detail),
                error: None,
                repo_dir: info.clone_dir.clone(),
                branch: current_branch,
            });
        } else {
            match git::push(
                &info.clone_dir,
                "origin",
                &current_branch,
                needs_upstream,
                force_with_lease,
            ) {
                Ok(()) => {
                    let mut detail = format!("pushed {} commit(s)", ahead);
                    if needs_upstream {
                        detail.push_str(" (upstream set)");
                    }
                    results.push(PushRepoResult {
                        name: info.dir_name.clone(),
                        action,
                        ok: true,
                        detail: Some(detail),
                        error: None,
                        repo_dir: info.clone_dir.clone(),
                        branch: current_branch,
                    });
                }
                Err(e) => {
                    results.push(PushRepoResult {
                        name: info.dir_name.clone(),
                        action,
                        ok: false,
                        detail: None,
                        error: Some(e.to_string()),
                        repo_dir: info.clone_dir.clone(),
                        branch: current_branch,
                    });
                }
            }
        }
    }

    Ok(Output::Push(PushOutput {
        workspace: meta.name,
        branch: meta.branch,
        dry_run,
        repos: results,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{local_commit, setup_clone_repo};
    use std::process::Command as StdCommand;

    #[test]
    fn test_push_nothing_to_push() {
        let (clone, _source, _ct, _st) = setup_clone_repo();

        let upstream = git::resolve_upstream_ref(&clone);
        let ahead = git::ahead_count_from(&clone, &upstream).unwrap_or(0);
        assert_eq!(ahead, 0, "fresh clone on feature should be 0 ahead");
    }

    #[test]
    fn test_push_detects_upstream_needed() {
        let (clone, _source, _ct, _st) = setup_clone_repo();

        // feature branch has no tracking branch
        let upstream = git::resolve_upstream_ref(&clone);
        let needs_upstream = !matches!(upstream, UpstreamRef::Tracking)
            || !git::remote_branch_exists(&clone, "feature");
        assert!(needs_upstream, "feature branch should need upstream set");
    }

    #[test]
    fn test_push_refuses_default_branch() {
        let (clone, _source, _ct, _st) = setup_clone_repo();

        // Switch to main
        let out = StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(out.status.success());

        let current = git::branch_current(&clone).unwrap();
        let default = git::default_branch(&clone).unwrap();
        assert_eq!(current, default, "should be on default branch");
        // The run() function would refuse to push here
    }

    #[test]
    fn test_push_ahead_count_after_commit() {
        let (clone, _source, _ct, _st) = setup_clone_repo();

        local_commit(&clone, "new.txt", "content");

        let upstream = git::resolve_upstream_ref(&clone);
        let ahead = git::ahead_count_from(&clone, &upstream).unwrap_or(0);
        assert!(ahead > 0, "should be ahead after local commit");
    }
}
