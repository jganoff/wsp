use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, bail};
use clap::{Arg, ArgAction, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use super::completers;
use crate::config::{self, Paths};
use crate::git::{self, SyncAction};
use crate::output::{Output, SyncOutput, SyncRepoResult};
use crate::workspace::{self, RepoInfo};

pub fn cmd() -> Command {
    Command::new("sync")
        .about("Fetch and rebase/merge all workspace repos")
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("strategy")
                .long("strategy")
                .value_parser(["rebase", "merge"])
                .help("Sync strategy: rebase (default) or merge"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Preview actions without executing"),
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

    let cfg = config::Config::load_from(&paths.config_path)?;
    let strategy = matches
        .get_one::<String>("strategy")
        .map(|s| s.as_str())
        .or(cfg.sync_strategy.as_deref())
        .unwrap_or("rebase");

    // Validate strategy (config file values bypass clap's value_parser)
    match strategy {
        "rebase" | "merge" => {}
        other => bail!(
            "invalid sync-strategy {:?} in config; must be 'rebase' or 'merge'",
            other
        ),
    }

    let dry_run = matches.get_flag("dry-run");

    let repo_infos = meta.repo_infos(&ws_dir);

    // Phase 1: Parallel fetch (skip if dry-run)
    let fetch_failures: HashSet<String> = if !dry_run {
        let progress = Mutex::new(());
        let fetchable: Vec<&RepoInfo> = repo_infos.iter().filter(|r| r.error.is_none()).collect();
        if !fetchable.is_empty() {
            eprintln!("Fetching {} repo(s)...", fetchable.len());
        }

        let results: Vec<(String, bool)> = std::thread::scope(|s| {
            let handles: Vec<_> = fetchable
                .iter()
                .map(|info| {
                    let progress = &progress;
                    s.spawn(move || {
                        let result = git::fetch_remote_prune(&info.clone_dir, "origin");
                        let _lock = progress.lock().unwrap_or_else(|e| e.into_inner());
                        match &result {
                            Ok(()) => eprintln!("  ok    {}", info.dir_name),
                            Err(e) => eprintln!("  FAIL  {} ({})", info.dir_name, e),
                        }
                        (info.dir_name.clone(), result.is_err())
                    })
                })
                .collect();

            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| (String::new(), true)))
                .collect()
        });

        results
            .into_iter()
            .filter(|(_, failed)| *failed)
            .map(|(name, _)| name)
            .collect()
    } else {
        HashSet::new()
    };

    // Phase 2: Serial sync
    let mut results = Vec::new();
    for info in &repo_infos {
        if let Some(ref e) = info.error {
            results.push(SyncRepoResult {
                name: info.dir_name.clone(),
                action: String::new(),
                ok: false,
                detail: None,
                error: Some(e.clone()),
                repo_dir: info.clone_dir.clone(),
                target: String::new(),
                strategy: strategy.to_string(),
            });
            continue;
        }

        let fetch_failed = fetch_failures.contains(&info.dir_name);

        if info.is_context {
            let pinned = info.pinned_ref.as_deref().unwrap_or("HEAD");
            let action = format!("checkout {}", pinned);
            if dry_run {
                results.push(SyncRepoResult {
                    name: info.dir_name.clone(),
                    action,
                    ok: true,
                    detail: Some("(dry run)".into()),
                    error: None,
                    repo_dir: info.clone_dir.clone(),
                    target: pinned.to_string(),
                    strategy: String::new(),
                });
            } else {
                match sync_context_repo(&info.clone_dir, pinned) {
                    Ok(mut detail) => {
                        if fetch_failed {
                            detail.push_str(" (fetch failed, data may be stale)");
                        }
                        results.push(SyncRepoResult {
                            name: info.dir_name.clone(),
                            action,
                            ok: true,
                            detail: Some(detail),
                            error: None,
                            repo_dir: info.clone_dir.clone(),
                            target: pinned.to_string(),
                            strategy: String::new(),
                        });
                    }
                    Err(e) => {
                        results.push(SyncRepoResult {
                            name: info.dir_name.clone(),
                            action,
                            ok: false,
                            detail: None,
                            error: Some(e.to_string()),
                            repo_dir: info.clone_dir.clone(),
                            target: pinned.to_string(),
                            strategy: String::new(),
                        });
                    }
                }
            }
        } else {
            // Active repo: resolve default branch first (used in all paths)
            let default_branch = match git::default_branch(&info.clone_dir) {
                Ok(b) => b,
                Err(e) => {
                    results.push(SyncRepoResult {
                        name: info.dir_name.clone(),
                        action: format!("{} onto origin/?", strategy),
                        ok: false,
                        detail: None,
                        error: Some(format!("cannot detect default branch: {}", e)),
                        repo_dir: info.clone_dir.clone(),
                        target: String::new(),
                        strategy: strategy.to_string(),
                    });
                    continue;
                }
            };
            let target = format!("origin/{}", default_branch);
            let action = format!("{} onto {}", strategy, target);

            // Check for dirty working tree
            let changed = git::changed_file_count(&info.clone_dir).unwrap_or(0);
            if changed > 0 {
                results.push(SyncRepoResult {
                    name: info.dir_name.clone(),
                    action,
                    ok: false,
                    detail: None,
                    error: Some(format!(
                        "uncommitted changes ({} file(s)), skipping",
                        changed
                    )),
                    repo_dir: info.clone_dir.clone(),
                    target,
                    strategy: strategy.to_string(),
                });
                continue;
            }

            if dry_run {
                let detail = describe_pending_sync(&info.clone_dir, &target);
                results.push(SyncRepoResult {
                    name: info.dir_name.clone(),
                    action,
                    ok: true,
                    detail: Some(detail),
                    error: None,
                    repo_dir: info.clone_dir.clone(),
                    target,
                    strategy: strategy.to_string(),
                });
            } else {
                match sync_active_repo(&info.clone_dir, &target, strategy) {
                    Ok(sync_action) => {
                        let mut detail = format_sync_action(&sync_action);
                        if fetch_failed {
                            detail.push_str(" (fetch failed, data may be stale)");
                        }
                        results.push(SyncRepoResult {
                            name: info.dir_name.clone(),
                            action,
                            ok: true,
                            detail: Some(detail),
                            error: None,
                            repo_dir: info.clone_dir.clone(),
                            target,
                            strategy: strategy.to_string(),
                        });
                    }
                    Err(_) => {
                        results.push(SyncRepoResult {
                            name: info.dir_name.clone(),
                            action,
                            ok: false,
                            detail: None,
                            error: Some("aborted, repo unchanged".into()),
                            repo_dir: info.clone_dir.clone(),
                            target,
                            strategy: strategy.to_string(),
                        });
                    }
                }
            }
        }
    }

    Ok(Output::Sync(SyncOutput {
        workspace: meta.name,
        branch: meta.branch,
        dry_run,
        repos: results,
    }))
}

fn sync_active_repo(dir: &Path, target: &str, strategy: &str) -> Result<SyncAction> {
    match strategy {
        "merge" => git::merge_from(dir, target),
        _ => git::rebase_onto(dir, target),
    }
}

fn sync_context_repo(dir: &Path, pinned_ref: &str) -> Result<String> {
    let origin_ref = format!("origin/{}", pinned_ref);

    // Check if origin/<ref> exists (branch tracking)
    if git::ref_exists(dir, &format!("refs/remotes/{}", origin_ref)) {
        // It's a branch — fast-forward the local branch
        match git::merge_from(dir, &origin_ref) {
            Ok(SyncAction::UpToDate) => Ok("already up to date".into()),
            Ok(SyncAction::FastForward { commits }) => {
                Ok(format!("fast-forwarded {} commit(s)", commits))
            }
            Ok(SyncAction::Merged) => Ok("merged".into()),
            Ok(SyncAction::Rebased { commits }) => Ok(format!("{} commit(s) rebased", commits)),
            Err(e) => Err(e),
        }
    } else {
        // Tag or SHA — ensure checkout is on the expected ref
        git::checkout(dir, pinned_ref)?;
        Ok("already up to date".into())
    }
}

fn format_sync_action(action: &SyncAction) -> String {
    match action {
        SyncAction::UpToDate => "already up to date".into(),
        SyncAction::FastForward { commits } => format!("fast-forwarded {} commit(s)", commits),
        SyncAction::Rebased { commits } => format!("{} commit(s) rebased", commits),
        SyncAction::Merged => "merged".into(),
    }
}

fn describe_pending_sync(dir: &Path, target: &str) -> String {
    let target_sha = git::run(Some(dir), &["rev-parse", target]).unwrap_or_default();
    let head_sha = git::run(Some(dir), &["rev-parse", "HEAD"]).unwrap_or_default();

    if target_sha.is_empty() || head_sha.is_empty() {
        return "(unknown)".into();
    }

    if target_sha == head_sha {
        return "already up to date".into();
    }

    let behind = git::commit_count(dir, "HEAD", target).unwrap_or(0);
    let ahead = git::commit_count(dir, target, "HEAD").unwrap_or(0);

    match (behind, ahead) {
        (0, 0) => "already up to date".into(),
        (b, 0) => format!("{} behind", b),
        (0, a) => format!("{} ahead", a),
        (b, a) => format!("{} behind, {} ahead", b, a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_sync_action() {
        let cases = vec![
            ("up to date", SyncAction::UpToDate, "already up to date"),
            (
                "fast forward 1",
                SyncAction::FastForward { commits: 1 },
                "fast-forwarded 1 commit(s)",
            ),
            (
                "fast forward 5",
                SyncAction::FastForward { commits: 5 },
                "fast-forwarded 5 commit(s)",
            ),
            (
                "rebased 3",
                SyncAction::Rebased { commits: 3 },
                "3 commit(s) rebased",
            ),
            ("merged", SyncAction::Merged, "merged"),
        ];
        for (name, action, want) in cases {
            assert_eq!(format_sync_action(&action), want, "{}", name);
        }
    }

    #[test]
    fn test_sync_blocks_dirty_working_tree() {
        let (clone_dir, _source, _ct, _st) = crate::testutil::setup_clone_repo();

        // Create dirty file
        std::fs::write(clone_dir.join("dirty.txt"), "dirty").unwrap();

        let changed = git::changed_file_count(&clone_dir).unwrap();
        assert!(changed > 0, "should have uncommitted changes");

        // The caller checks changed > 0 and skips the repo — verify that invariant
    }

    #[test]
    fn test_sync_continues_after_conflict() {
        use crate::testutil::{local_commit, setup_clone_repo};
        use std::process::Command as StdCommand;

        // First clone provides the shared source repo
        let (clone1, source, _ct1, _st1) = setup_clone_repo();

        // Second clone from the same source
        let clone2_tmp = tempfile::tempdir().unwrap();
        let clone2 = clone2_tmp.path().join("repo2");
        let out = StdCommand::new("git")
            .args(["clone", source.to_str().unwrap(), clone2.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success());
        for args in &[
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec![
                "git",
                "checkout",
                "-b",
                "feature",
                "--no-track",
                "origin/main",
            ],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(&clone2)
                .output()
                .unwrap();
            assert!(out.status.success());
        }

        // Add upstream commit that conflicts with clone1
        local_commit(&source, "conflict.txt", "upstream version");

        // Fetch in both clones
        git::fetch_remote_prune(&clone1, "origin").unwrap();
        git::fetch_remote_prune(&clone2, "origin").unwrap();

        // Add conflicting local commit in clone1
        local_commit(&clone1, "conflict.txt", "local version");

        // Sync clone1 — should fail (conflict)
        let result1 = sync_active_repo(&clone1, "origin/main", "rebase");
        assert!(result1.is_err(), "clone1 should have conflict");

        // Sync clone2 — should succeed (no local changes, just fast-forward)
        let result2 = sync_active_repo(&clone2, "origin/main", "rebase");
        assert!(result2.is_ok(), "clone2 should sync successfully");
        assert_eq!(result2.unwrap(), SyncAction::FastForward { commits: 1 });
    }
}
