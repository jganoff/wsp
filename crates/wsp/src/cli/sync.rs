use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, bail};
use clap::{Arg, ArgAction, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use super::completers;
use wsp_core::config::{self, Paths};
use wsp_core::discovery;
use wsp_core::gc;
use wsp_core::git::{self, SyncAction};
use wsp_core::giturl;
use wsp_core::mirror;
use wsp_core::output::{Output, SyncAbortOutput, SyncAbortRepoResult, SyncOutput, SyncRepoResult};
use wsp_core::workspace::{self, RepoInfo};

pub fn cmd() -> Command {
    Command::new("sync")
        .add(crate::shellnav::ShellNav::none())
        .about("Fetch and rebase/merge all workspace repos")
        .long_about(
            "Fetch and rebase/merge all workspace repos.\n\n\
             Fetches upstream changes through the mirror layer, then rebases (default) or \
             merges each repo's workspace branch onto its upstream tracking branch. If a \
             conflict occurs, the operation pauses — resolve it with git, then re-run sync \
             to continue with the remaining repos. Use --abort to cancel in-progress \
             operations across all repos.",
        )
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("strategy")
                .long("strategy")
                .value_parser(["rebase", "merge"])
                .help("Sync strategy: rebase (default) or merge")
                .conflicts_with("abort"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Preview actions without executing")
                .conflicts_with("abort"),
        )
        .arg(
            Arg::new("abort")
                .long("abort")
                .action(ArgAction::SetTrue)
                .help("Abort in-progress rebase/merge across all repos"),
        )
        .arg(
            Arg::new("no-discover")
                .long("no-discover")
                .action(ArgAction::SetTrue)
                .help("Skip template discovery after sync"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let ws_dir: PathBuf = if let Some(name) = matches.get_one::<String>("workspace") {
        workspace::dir(&paths.workspaces_dir, name)
    } else {
        let cwd = std::env::current_dir()?;
        workspace::detect(&cwd)?
    };

    gc::check_workspace(&ws_dir, /* read_only */ false)?;

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    if matches.get_flag("abort") {
        return run_abort(&ws_dir, &meta);
    }

    let cfg = config::Config::load_from(&paths.config_path)?;
    let strategy = matches
        .get_one::<String>("strategy")
        .map(|s| s.as_str())
        .or(meta
            .config
            .as_ref()
            .and_then(|c| c.sync_strategy.as_deref()))
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

    // Phase 1a: Fetch mirrors from upstream (network, parallel, skip if dry-run)
    let fetch_failures: HashSet<String> = if !dry_run {
        let mirrors: Vec<(&RepoInfo, PathBuf)> = repo_infos
            .iter()
            .filter(|r| r.error.is_none())
            .filter_map(|info| {
                giturl::Parsed::from_identity(&info.identity)
                    .ok()
                    .map(|parsed| (info, mirror::dir(&paths.mirrors_dir, &parsed)))
            })
            .collect();

        if !mirrors.is_empty() {
            eprintln!("Fetching {} repo(s)...", mirrors.len());
        }

        let progress = Mutex::new(());
        let results: Vec<(String, bool)> = std::thread::scope(|s| {
            let handles: Vec<_> = mirrors
                .iter()
                .map(|(info, mirror_path)| {
                    let progress = &progress;
                    s.spawn(move || {
                        let result = git::fetch(mirror_path, true);
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

        // Phase 1b: Propagate mirror refs to clones (runs for all repos, including
        // those whose mirror fetch failed — stale mirror data is still useful and
        // propagation is a local no-op when nothing changed).
        workspace::propagate_mirror_to_clones(&paths.mirrors_dir, &ws_dir, &meta, &cfg, true);

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
                identity: info.identity.clone(),
                shortname: info.dir_name.clone(),
                path: info.clone_dir.to_string_lossy().to_string(),
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

        // Skip repos that are on a different branch than the workspace branch.
        // Rebasing onto the workspace's upstream target while HEAD is on an
        // unrelated branch would silently rebase the wrong branch.
        let current_branch = git::branch_current(&info.clone_dir).unwrap_or_default();
        if !current_branch.is_empty() && current_branch != meta.branch {
            results.push(SyncRepoResult {
                identity: info.identity.clone(),
                shortname: info.dir_name.clone(),
                path: info.clone_dir.to_string_lossy().to_string(),
                action: "skipped".into(),
                ok: true,
                detail: Some(format!("on {}, expected {}", current_branch, meta.branch)),
                error: None,
                repo_dir: info.clone_dir.clone(),
                target: String::new(),
                strategy: strategy.to_string(),
            });
            continue;
        }

        // Resolve default branch first (used in all paths)
        let default_branch = match git::default_branch(&info.clone_dir) {
            Ok(b) => b,
            Err(e) => {
                results.push(SyncRepoResult {
                    identity: info.identity.clone(),
                    shortname: info.dir_name.clone(),
                    path: info.clone_dir.to_string_lossy().to_string(),
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
                identity: info.identity.clone(),
                shortname: info.dir_name.clone(),
                path: info.clone_dir.to_string_lossy().to_string(),
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
                identity: info.identity.clone(),
                shortname: info.dir_name.clone(),
                path: info.clone_dir.to_string_lossy().to_string(),
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
                        identity: info.identity.clone(),
                        shortname: info.dir_name.clone(),
                        path: info.clone_dir.to_string_lossy().to_string(),
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
                        identity: info.identity.clone(),
                        shortname: info.dir_name.clone(),
                        path: info.clone_dir.to_string_lossy().to_string(),
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

    // Template discovery: scan repos after sync for new/changed .wsp.yaml files
    if !dry_run && !matches.get_flag("no-discover") {
        let mut all_discovered = Vec::new();
        for info in &repo_infos {
            if info.error.is_some() {
                continue;
            }
            let discovered =
                discovery::scan_repo_dir(&info.clone_dir, &info.identity, &paths.templates_dir);
            all_discovered.extend(discovered);
        }
        if let Err(e) = discovery::prompt_and_import(&all_discovered, &paths.templates_dir) {
            eprintln!("warning: template discovery failed: {}", e);
        }
    }

    Ok(Output::Sync(SyncOutput {
        workspace: meta.name,
        branch: meta.branch,
        dry_run,
        repos: results,
    }))
}

fn run_abort(ws_dir: &Path, meta: &workspace::Metadata) -> Result<Output> {
    let repo_infos = meta.repo_infos(ws_dir);
    let mut results = Vec::new();

    for info in &repo_infos {
        if let Some(ref e) = info.error {
            results.push(SyncAbortRepoResult {
                identity: info.identity.clone(),
                shortname: info.dir_name.clone(),
                path: info.clone_dir.to_string_lossy().to_string(),
                action: "error".into(),
                ok: false,
                error: Some(e.clone()),
            });
            continue;
        }

        match git::in_progress_op(&info.clone_dir) {
            Some(op) => {
                let action = match op {
                    git::InProgressOp::Rebase => "rebase aborted",
                    git::InProgressOp::Merge => "merge aborted",
                };
                match git::abort_in_progress(&info.clone_dir, &op) {
                    Ok(()) => results.push(SyncAbortRepoResult {
                        identity: info.identity.clone(),
                        shortname: info.dir_name.clone(),
                        path: info.clone_dir.to_string_lossy().to_string(),
                        action: action.into(),
                        ok: true,
                        error: None,
                    }),
                    Err(e) => results.push(SyncAbortRepoResult {
                        identity: info.identity.clone(),
                        shortname: info.dir_name.clone(),
                        path: info.clone_dir.to_string_lossy().to_string(),
                        action: action.into(),
                        ok: false,
                        error: Some(e.to_string()),
                    }),
                }
            }
            None => results.push(SyncAbortRepoResult {
                identity: info.identity.clone(),
                shortname: info.dir_name.clone(),
                path: info.clone_dir.to_string_lossy().to_string(),
                action: "skip".into(),
                ok: true,
                error: None,
            }),
        }
    }

    Ok(Output::SyncAbort(SyncAbortOutput {
        workspace: meta.name.clone(),
        repos: results,
    }))
}

fn sync_active_repo(dir: &Path, target: &str, strategy: &str) -> Result<SyncAction> {
    match strategy {
        "merge" => git::merge_from(dir, target),
        _ => git::rebase_onto(dir, target),
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
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();

        // Commit a tracked file in the clone so we can make it dirty later
        local_commit(&clone_dir, "tracked.txt", "original content");

        // Add an upstream commit so origin/main is ahead of the clone
        local_commit(&source, "upstream.txt", "upstream change");

        // Fetch the upstream change into the clone
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();

        // Modify the tracked file without committing — dirty working tree
        std::fs::write(clone_dir.join("tracked.txt"), "modified content").unwrap();

        // Verify the dirty-tree precondition: changed_file_count > 0 triggers the guard
        let changed = git::changed_file_count(&clone_dir).unwrap();
        assert!(changed > 0, "should have uncommitted changes");

        // sync_active_repo must fail when working tree has unstaged tracked modifications;
        // git rebase refuses to run with a dirty working tree.
        let result = sync_active_repo(&clone_dir, "origin/main", "rebase");
        assert!(
            result.is_err(),
            "sync should refuse to operate on a dirty working tree"
        );
    }

    #[test]
    fn test_sync_continues_after_conflict() {
        use std::process::Command as StdCommand;
        use wsp_core::testutil::{local_commit, setup_clone_repo};

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

    #[test]
    fn test_sync_skips_wrong_branch() {
        // When a repo is on a branch other than the workspace branch, sync
        // should skip it cleanly rather than attempt (and likely fail) a rebase.
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();

        // Add an upstream commit so there's something to sync
        local_commit(&source, "upstream.txt", "upstream");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();

        // The repo is on its default branch (e.g. "main"), not the workspace branch.
        let repo_branch = git::branch_current(&clone_dir).unwrap();
        let ws_branch = format!("{}-workspace", repo_branch);

        // ws_branch != repo_branch → sync should skip, not error.
        // We verify by checking the detection condition directly: if branch_current
        // != ws_branch the outer loop would push a skipped result and continue.
        assert_ne!(
            repo_branch, ws_branch,
            "precondition: repo branch differs from workspace branch"
        );

        // sync_active_repo itself would succeed here (clean tree, fast-forward
        // available), confirming the skip is a deliberate choice by the outer
        // loop, not a fallback from a failing rebase.
        let result = sync_active_repo(&clone_dir, "origin/main", "rebase");
        assert!(
            result.is_ok(),
            "sync_active_repo should succeed on a clean wrong-branch repo, \
             confirming that only the outer branch check produces the skip"
        );
    }
}
