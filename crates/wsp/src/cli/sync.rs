use std::collections::{HashMap, HashSet};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, bail};
use clap::{Arg, ArgAction, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use super::{completers, fetch};
use wsp_core::config::{self, Paths};
use wsp_core::discovery;
use wsp_core::gc;
use wsp_core::git::{self, SyncAction};
use wsp_core::giturl;
use wsp_core::mirror;
use wsp_core::output::{
    Output, SyncAbortOutput, SyncAbortRepoResult, SyncOutput, SyncRepoResult, SyncRepoStatus,
};
use wsp_core::workspace::{self, RepoInfo};

pub fn cmd() -> Command {
    Command::new("sync")
        .add(crate::shellnav::ShellNav::none())
        .about("Fetch and rebase/merge all workspace repos")
        .long_about(
            "Fetch and rebase/merge all workspace repos.\n\n\
             If a conflict occurs, the repo is left mid-rebase/merge and sync continues \
             with the remaining repos. Resolve conflicts with git, then run `wsp sync` \
             again to resume all in-progress operations and sync the remaining repos. \
             Use --abort to cancel all in-progress operations.",
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
            Arg::new("yes")
                .short('y')
                .long("yes")
                .action(ArgAction::SetTrue)
                .help("Skip confirmation prompt when aborting operations")
                .requires("abort"),
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
        let cwd = crate::shellcd::invocation_dir()?;
        workspace::detect(&cwd)?
    };

    gc::check_workspace(&ws_dir, /* read_only */ false)?;

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    if matches.get_flag("abort") {
        return run_abort(&ws_dir, &meta, matches.get_flag("yes"));
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
    let no_discover = matches.get_flag("no-discover");

    if !dry_run {
        return run_live(&ws_dir, &meta, &cfg, strategy, paths, no_discover);
    }

    let repo_infos = meta.repo_infos(&ws_dir);

    // PHASE 0 — PRE-FLIGHT: detect mid-flight repos from prior session.
    //
    // Dry runs report mid-flight repos as paused. Clean repos are still previewed.
    // This phase MUST run before the dirty-tree guard in sync_one_repo because
    // mid-rebase repos show unmerged paths which would otherwise be misclassified
    // as "dirty working tree".
    let mid_flight = detect_mid_flight(&repo_infos);
    let mid_flight_names: HashSet<_> = mid_flight.iter().map(|(n, _)| n.clone()).collect();

    let mut results = Vec::new();
    for (name, op) in &mid_flight {
        let info = repo_infos
            .iter()
            .find(|i| &i.dir_name == name)
            .expect("mid-flight repo must be present in repo_infos");
        let op_name = match op {
            git::InProgressOp::Rebase => "rebase",
            git::InProgressOp::Merge => "merge",
        };
        results.push(SyncRepoResult {
            identity: info.identity.clone(),
            shortname: info.dir_name.clone(),
            path: info.clone_dir.to_string_lossy().to_string(),
            action: format!("paused — in-progress {}", op_name),
            status: SyncRepoStatus::Paused,
            detail: None,
            error: Some("resolve conflicts and run `wsp sync` again".to_string()),
            repo_dir: info.clone_dir.clone(),
            target: String::new(),
            strategy: op_name.to_string(),
        });
    }

    // PHASE 1 — SYNC: preview non-mid-flight repos via the shared guard chain.
    for info in repo_infos
        .iter()
        .filter(|i| !mid_flight_names.contains(&i.dir_name))
    {
        results.push(sync_one_repo(info, &meta, dry_run, strategy));
    }

    // PHASE 2 — POST-SYNC: template discovery.
    // Scans repos for new/changed .wsp.yaml files after sync completes.
    if !dry_run && !no_discover {
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

fn run_abort(ws_dir: &Path, meta: &workspace::Metadata, yes: bool) -> Result<Output> {
    let repo_infos = meta.repo_infos(ws_dir);
    let operations: Vec<Option<git::InProgressOp>> = repo_infos
        .iter()
        .map(|info| git::in_progress_op(&info.clone_dir))
        .collect();
    let has_operations = operations.iter().any(Option::is_some);
    require_abort_confirmation(has_operations, yes, std::io::stdin().is_terminal())?;
    if has_operations && !yes {
        eprint!("Abort all in-progress rebase/merge operations? [y/N] ");
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.is_empty() || !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            bail!("aborted");
        }
    }
    let mut results = Vec::new();

    for (info, operation) in repo_infos.iter().zip(operations) {
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

        match operation {
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

fn require_abort_confirmation(has_operations: bool, yes: bool, stdin_is_tty: bool) -> Result<()> {
    if !has_operations || yes || stdin_is_tty {
        return Ok(());
    }
    bail!("pass --yes to confirm: wsp sync --abort --yes")
}

/// Fetch, continue mid-flight repos, sync clean repos, and discover templates.
fn run_live(
    ws_dir: &Path,
    meta: &workspace::Metadata,
    cfg: &config::Config,
    strategy: &str,
    paths: &Paths,
    no_discover: bool,
) -> Result<Output> {
    let repo_infos = meta.repo_infos(ws_dir);

    // PHASE 1 — FETCH: refresh upstream data.
    // Upstream may have moved while the user was resolving conflicts.
    let fetch_failures = fetch_workspace_mirrors(&repo_infos, paths, meta, cfg, ws_dir);

    // PHASE 2 — RESUME mid-flight repos and SYNC clean repos.
    //
    // Ordering invariant: in_progress_op check runs BEFORE sync_one_repo so that
    // the dirty-tree guard in sync_one_repo is never triggered by unmerged paths.
    let mut results = Vec::new();
    for info in &repo_infos {
        results.push(sync_repo_after_fetch(
            info,
            meta,
            strategy,
            fetch_failures.get(&info.dir_name).map(String::as_str),
        ));
    }

    // PHASE 3 — POST-SYNC: template discovery.
    if !no_discover {
        let mut all_discovered = Vec::new();
        let discoverable = discoverable_repo_indices(
            &results.iter().map(|r| r.status.clone()).collect::<Vec<_>>(),
        );
        for index in discoverable {
            let info = &repo_infos[index];
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
        workspace: meta.name.clone(),
        branch: meta.branch.clone(),
        dry_run: false,
        repos: results,
    }))
}

/// Ask Git to continue the operation it reports as in progress.
fn sync_repo_after_fetch(
    info: &RepoInfo,
    meta: &workspace::Metadata,
    strategy: &str,
    fetch_error: Option<&str>,
) -> SyncRepoResult {
    if let Some(error) = fetch_error {
        return SyncRepoResult {
            identity: info.identity.clone(),
            shortname: info.dir_name.clone(),
            path: info.clone_dir.to_string_lossy().to_string(),
            action: "sync skipped".into(),
            status: SyncRepoStatus::Failed,
            detail: None,
            error: Some(format!("mirror refresh failed: {error}")),
            repo_dir: info.clone_dir.clone(),
            target: String::new(),
            strategy: strategy.to_string(),
        };
    }
    match git::in_progress_op(&info.clone_dir) {
        Some(op) => resume_repo(info, op, &meta.branch),
        None => sync_one_repo(info, meta, false, strategy),
    }
}

fn resume_repo(info: &RepoInfo, op: git::InProgressOp, expected_branch: &str) -> SyncRepoResult {
    let strategy = match op {
        git::InProgressOp::Rebase => "rebase",
        git::InProgressOp::Merge => "merge",
    };
    let result = git::in_progress_branch(&info.clone_dir, &op);
    let action = format!("{strategy} --continue");

    match result {
        Ok(branch) if branch == expected_branch => {
            let result = match op {
                git::InProgressOp::Rebase => git::rebase_continue(&info.clone_dir),
                git::InProgressOp::Merge => git::merge_continue(&info.clone_dir),
            };
            match result {
                Ok(sync_action) => SyncRepoResult {
                    identity: info.identity.clone(),
                    shortname: info.dir_name.clone(),
                    path: info.clone_dir.to_string_lossy().to_string(),
                    action,
                    status: SyncRepoStatus::Ok,
                    detail: Some(format_sync_action(&sync_action)),
                    error: None,
                    repo_dir: info.clone_dir.clone(),
                    target: String::new(),
                    strategy: strategy.to_string(),
                },
                Err(error) => continuation_failure_result(info, strategy, action, error),
            }
        }
        Ok(branch) => SyncRepoResult {
            identity: info.identity.clone(),
            shortname: info.dir_name.clone(),
            path: info.clone_dir.to_string_lossy().to_string(),
            action,
            status: SyncRepoStatus::Failed,
            detail: None,
            error: Some(format!(
                "in-progress {strategy} is on {branch}, expected {expected_branch}; leaving it untouched"
            )),
            repo_dir: info.clone_dir.clone(),
            target: String::new(),
            strategy: strategy.to_string(),
        },
        Err(error) => SyncRepoResult {
            identity: info.identity.clone(),
            shortname: info.dir_name.clone(),
            path: info.clone_dir.to_string_lossy().to_string(),
            action,
            status: SyncRepoStatus::Failed,
            detail: None,
            error: Some(format!(
                "cannot determine in-progress {strategy} branch: {error:#}"
            )),
            repo_dir: info.clone_dir.clone(),
            target: String::new(),
            strategy: strategy.to_string(),
        },
    }
}

fn continuation_failure_result(
    info: &RepoInfo,
    strategy: &str,
    action: String,
    error: anyhow::Error,
) -> SyncRepoResult {
    let status = classify_continue_failure(&info.clone_dir);
    let paused = matches!(status, SyncRepoStatus::Paused);
    SyncRepoResult {
        identity: info.identity.clone(),
        shortname: info.dir_name.clone(),
        path: info.clone_dir.to_string_lossy().to_string(),
        action,
        status,
        detail: None,
        error: Some(if paused {
            format!("{strategy} still has conflicts — stage resolutions and retry")
        } else {
            format!("{strategy} --continue failed: {error:#}")
        }),
        repo_dir: info.clone_dir.clone(),
        target: String::new(),
        strategy: strategy.to_string(),
    }
}

fn classify_continue_failure(dir: &Path) -> SyncRepoStatus {
    match git::has_unmerged_paths(dir) {
        Ok(true) => SyncRepoStatus::Paused,
        Ok(false) | Err(_) => SyncRepoStatus::Failed,
    }
}

fn discoverable_repo_indices(statuses: &[SyncRepoStatus]) -> Vec<usize> {
    statuses
        .iter()
        .enumerate()
        .filter_map(|(index, status)| matches!(status, SyncRepoStatus::Ok).then_some(index))
        .collect()
}

/// Fetch all workspace mirrors from upstream and propagate refs to clones.
///
/// Returns each repo `dir_name` whose mirror fetch failed and its error message.
fn fetch_workspace_mirrors(
    repo_infos: &[RepoInfo],
    paths: &Paths,
    meta: &workspace::Metadata,
    cfg: &config::Config,
    ws_dir: &Path,
) -> HashMap<String, String> {
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

    let results: Vec<(String, Option<String>)> = if mirrors.len() > 1 && io::stderr().is_terminal()
    {
        let inputs: Vec<(String, PathBuf)> = mirrors
            .iter()
            .map(|(info, mirror_path)| (info.dir_name.clone(), mirror_path.clone()))
            .collect();
        fetch::fetch_mirrors_with_progress(&inputs, true)
            .into_iter()
            .map(|(name, result)| {
                match &result {
                    Ok(()) => eprintln!("  ok    {}", name),
                    Err(e) => eprintln!("  FAIL  {} ({})", name, e),
                }
                (name, result.err().map(|e| e.to_string()))
            })
            .collect()
    } else if mirrors.len() == 1 && io::stderr().is_terminal() {
        let (info, mirror_path) = &mirrors[0];
        let result = git::fetch_with_progress(mirror_path, true);
        match &result {
            Ok(()) => eprintln!("  ok    {}", info.dir_name),
            Err(e) => eprintln!("  FAIL  {} ({})", info.dir_name, e),
        }
        vec![(info.dir_name.clone(), result.err().map(|e| e.to_string()))]
    } else {
        let progress = Mutex::new(());
        std::thread::scope(|s| {
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
                        (info.dir_name.clone(), result.err().map(|e| e.to_string()))
                    })
                })
                .collect();

            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| (String::new(), Some("fetch worker panicked".into())))
                })
                .collect()
        })
    };

    // Propagate mirror refs to clones (runs for all repos, including those whose
    // mirror fetch failed — stale mirror data is still useful and propagation is
    // a local no-op when nothing changed).
    workspace::propagate_mirror_to_clones(&paths.mirrors_dir, ws_dir, meta, cfg, true);

    results
        .into_iter()
        .filter_map(|(name, error)| error.map(|error| (name, error)))
        .collect()
}

/// Detect repos with in-progress rebase or merge operations from a prior session.
///
/// Returns a vec of `(dir_name, op)` for each repo that has an in-progress operation.
/// Repos with errors (e.g., missing clone directory) are skipped.
fn detect_mid_flight(repo_infos: &[RepoInfo]) -> Vec<(String, git::InProgressOp)> {
    repo_infos
        .iter()
        .filter(|info| info.error.is_none())
        .filter_map(|info| {
            git::in_progress_op(&info.clone_dir).map(|op| (info.dir_name.clone(), op))
        })
        .collect()
}

/// Sync a single repo through the full guard chain.
///
/// Guards (in order):
/// 1. `info.error` — repo config error → status: Failed
/// 2. Branch check — not on workspace branch → status: Ok, action: "skipped"
/// 3. Default branch resolution → status: Failed on error
/// 4. Dirty working tree → status: Failed
/// 5. Dry-run preview → status: Ok with pending description
/// 6. Sync dispatch → status: Ok on success; on error: Paused if still
///    mid-flight (conflict), Failed if not (hard error).
///
/// **Ordering invariant**: call this only for repos where `in_progress_op`
/// returns `None` (verified by the caller). Mid-rebase repos have unmerged
/// paths that would trigger the dirty-tree guard and produce a misleading error.
fn sync_one_repo(
    info: &RepoInfo,
    meta: &workspace::Metadata,
    dry_run: bool,
    strategy: &str,
) -> SyncRepoResult {
    // Guard 1: repo config error
    if let Some(ref e) = info.error {
        return SyncRepoResult {
            identity: info.identity.clone(),
            shortname: info.dir_name.clone(),
            path: info.clone_dir.to_string_lossy().to_string(),
            action: String::new(),
            status: SyncRepoStatus::Failed,
            detail: None,
            error: Some(e.clone()),
            repo_dir: info.clone_dir.clone(),
            target: String::new(),
            strategy: strategy.to_string(),
        };
    }

    // Guard 2: branch check — skip repos on a different branch than the workspace branch.
    // Rebasing onto the workspace's upstream target while HEAD is on an unrelated branch
    // would silently rebase the wrong branch.
    let current_branch = git::branch_current(&info.clone_dir).unwrap_or_default();
    if !current_branch.is_empty() && current_branch != meta.branch {
        return SyncRepoResult {
            identity: info.identity.clone(),
            shortname: info.dir_name.clone(),
            path: info.clone_dir.to_string_lossy().to_string(),
            action: "skipped".into(),
            status: SyncRepoStatus::Ok,
            detail: Some(format!("on {}, expected {}", current_branch, meta.branch)),
            error: None,
            repo_dir: info.clone_dir.clone(),
            target: String::new(),
            strategy: strategy.to_string(),
        };
    }

    // Guard 3: resolve default branch (used in all remaining paths)
    let default_branch = match git::default_branch(&info.clone_dir) {
        Ok(b) => b,
        Err(e) => {
            return SyncRepoResult {
                identity: info.identity.clone(),
                shortname: info.dir_name.clone(),
                path: info.clone_dir.to_string_lossy().to_string(),
                action: format!("{} onto origin/?", strategy),
                status: SyncRepoStatus::Failed,
                detail: None,
                error: Some(format!("cannot detect default branch: {}", e)),
                repo_dir: info.clone_dir.clone(),
                target: String::new(),
                strategy: strategy.to_string(),
            };
        }
    };
    let target = format!("origin/{}", default_branch);
    let action = format!("{} onto {}", strategy, target);

    // Guard 4: dirty working tree
    let changed = git::changed_file_count(&info.clone_dir).unwrap_or(0);
    if changed > 0 {
        return SyncRepoResult {
            identity: info.identity.clone(),
            shortname: info.dir_name.clone(),
            path: info.clone_dir.to_string_lossy().to_string(),
            action,
            status: SyncRepoStatus::Failed,
            detail: None,
            error: Some(format!(
                "uncommitted changes ({} file(s)), skipping",
                changed
            )),
            repo_dir: info.clone_dir.clone(),
            target,
            strategy: strategy.to_string(),
        };
    }

    // Guard 5: dry-run preview
    if dry_run {
        let detail = describe_pending_sync(&info.clone_dir, &target);
        return SyncRepoResult {
            identity: info.identity.clone(),
            shortname: info.dir_name.clone(),
            path: info.clone_dir.to_string_lossy().to_string(),
            action,
            status: SyncRepoStatus::Ok,
            detail: Some(detail),
            error: None,
            repo_dir: info.clone_dir.clone(),
            target,
            strategy: strategy.to_string(),
        };
    }

    // Guard 6: sync dispatch + conflict classification
    match sync_active_repo(&info.clone_dir, &target, strategy) {
        Ok(sync_action) => {
            let detail = format_sync_action(&sync_action);
            SyncRepoResult {
                identity: info.identity.clone(),
                shortname: info.dir_name.clone(),
                path: info.clone_dir.to_string_lossy().to_string(),
                action,
                status: SyncRepoStatus::Ok,
                detail: Some(detail),
                error: None,
                repo_dir: info.clone_dir.clone(),
                target,
                strategy: strategy.to_string(),
            }
        }
        Err(e) => {
            // Classify the error: if git left an in-progress operation (rebase-merge or
            // MERGE_HEAD), the sync paused on a conflict and the user can resolve it.
            // If no in-progress op exists, it was a hard error (network, missing ref, etc.).
            if git::in_progress_op(&info.clone_dir).is_some() {
                SyncRepoResult {
                    identity: info.identity.clone(),
                    shortname: info.dir_name.clone(),
                    path: info.clone_dir.to_string_lossy().to_string(),
                    action,
                    status: SyncRepoStatus::Paused,
                    detail: None,
                    error: Some("conflict — resolve and run `wsp sync` again".to_string()),
                    repo_dir: info.clone_dir.clone(),
                    target,
                    strategy: strategy.to_string(),
                }
            } else {
                SyncRepoResult {
                    identity: info.identity.clone(),
                    shortname: info.dir_name.clone(),
                    path: info.clone_dir.to_string_lossy().to_string(),
                    action,
                    status: SyncRepoStatus::Failed,
                    detail: None,
                    error: Some(format!("sync failed: {e:#}")),
                    repo_dir: info.clone_dir.clone(),
                    target,
                    strategy: strategy.to_string(),
                }
            }
        }
    }
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
        SyncAction::Resumed { commits } => format!("resumed, {} commit(s) applied", commits),
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
            (
                "resumed 2",
                SyncAction::Resumed { commits: 2 },
                "resumed, 2 commit(s) applied",
            ),
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
    fn test_sync_conflict_leaves_repo_mid_flight_and_continues() {
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

        // Sync clone1 — should fail (conflict), and in_progress_op should be Some(Rebase)
        let result1 = sync_active_repo(&clone1, "origin/main", "rebase");
        assert!(result1.is_err(), "clone1 should have conflict");
        // Step 3: verify the repo is left mid-flight (not auto-aborted)
        assert_eq!(
            git::in_progress_op(&clone1),
            Some(git::InProgressOp::Rebase),
            "clone1 should be left mid-rebase after conflict"
        );

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

    #[test]
    fn test_detect_mid_flight_finds_mid_rebase_repo() {
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();

        // Add upstream commit that will conflict with our local commit
        local_commit(&source, "conflict.txt", "upstream version");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();
        local_commit(&clone_dir, "conflict.txt", "local version");

        // Trigger a conflict — repo enters mid-rebase state
        let result = sync_active_repo(&clone_dir, "origin/main", "rebase");
        assert!(result.is_err(), "should conflict");

        // Build a RepoInfo pointing at clone_dir
        let repo_info = RepoInfo {
            identity: "test.local/user/repo".to_string(),
            dir_name: "repo".to_string(),
            clone_dir: clone_dir.clone(),
            error: None,
        };

        // detect_mid_flight should find the mid-rebase repo
        let mid_flight = detect_mid_flight(&[repo_info]);
        assert_eq!(mid_flight.len(), 1, "should detect 1 mid-flight repo");
        assert_eq!(mid_flight[0].0, "repo");
        assert_eq!(mid_flight[0].1, git::InProgressOp::Rebase);
    }

    #[test]
    fn test_detect_mid_flight_empty_when_all_clean() {
        use wsp_core::testutil::setup_clone_repo;

        let (clone_dir, _source, _ct, _st) = setup_clone_repo();

        let repo_info = RepoInfo {
            identity: "test.local/user/repo".to_string(),
            dir_name: "repo".to_string(),
            clone_dir: clone_dir.clone(),
            error: None,
        };

        let mid_flight = detect_mid_flight(&[repo_info]);
        assert!(
            mid_flight.is_empty(),
            "should detect no mid-flight repos for clean repo"
        );
    }

    #[test]
    fn test_detect_mid_flight_skips_error_repos() {
        // Repos with errors (e.g., invalid clone_dir) should be skipped safely
        let error_info = RepoInfo {
            identity: "test.local/user/repo".to_string(),
            dir_name: "repo".to_string(),
            clone_dir: std::path::PathBuf::from("/nonexistent/path"),
            error: Some("dir not found".to_string()),
        };

        let mid_flight = detect_mid_flight(&[error_info]);
        assert!(
            mid_flight.is_empty(),
            "error repos should be excluded from mid-flight detection"
        );
    }

    /// Build a minimal Metadata with a single repo entry.
    ///
    /// `branch` is the workspace branch. `identity` is stored in `repos` so that
    /// `repo_infos` can resolve the dir_name, but the test only uses the fields
    /// directly for the `sync_one_repo` call (via a hand-built RepoInfo).
    fn make_test_meta(branch: &str) -> workspace::Metadata {
        workspace::Metadata {
            version: 0,
            name: "test-workspace".into(),
            branch: branch.into(),
            repos: std::collections::BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn test_sync_one_repo_skips_wrong_branch() {
        // sync_one_repo must skip cleanly when the repo is on a different branch
        // than the workspace branch. This validates that Guard 2 is present in
        // the extracted helper, not just in the old inline sync loop.
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();

        // Add an upstream commit so there is something to sync (rules out
        // a trivial up-to-date short-circuit masking a missing branch guard).
        local_commit(&source, "upstream.txt", "upstream");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();

        // setup_clone_repo leaves the clone on the "feature" branch.
        let current = git::branch_current(&clone_dir).unwrap();
        assert_eq!(
            current, "feature",
            "precondition: clone is on feature branch"
        );

        // Workspace branch is "main" — deliberately differs from "feature".
        let meta = make_test_meta("main");

        let info = RepoInfo {
            identity: "test.local/user/repo".into(),
            dir_name: "repo".into(),
            clone_dir: clone_dir.clone(),
            error: None,
        };

        let result = sync_one_repo(&info, &meta, false, "rebase");

        // Branch guard (Guard 2): status Ok, action "skipped", detail names both branches.
        assert_eq!(
            result.status,
            SyncRepoStatus::Ok,
            "wrong-branch repo should be Ok"
        );
        assert_eq!(result.action, "skipped", "action should be 'skipped'");
        let detail = result
            .detail
            .expect("detail should be set for wrong-branch skip");
        assert!(
            detail.contains("feature"),
            "detail should mention current branch; got: {:?}",
            detail
        );
        assert!(
            detail.contains("main"),
            "detail should mention workspace branch; got: {:?}",
            detail
        );
    }

    #[test]
    fn test_sync_one_repo_syncs_clean_repo() {
        // sync_one_repo on a clean repo behind origin should fast-forward and
        // return status Ok. This validates the happy-path through all guards.
        //
        // Note: rebase_continue / merge_continue at the git layer are fully covered
        // by git.rs::test_rebase_continue and test_merge_continue. This test focuses
        // on the sync_one_repo wrapper path where no in-progress op exists.
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();

        // setup_clone_repo leaves the clone on "feature". Use that as workspace branch
        // so Guard 2 passes.
        let meta = make_test_meta("feature");

        // Add an upstream commit so there is a fast-forward to pick up.
        local_commit(&source, "upstream.txt", "upstream content");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();

        let info = RepoInfo {
            identity: "test.local/user/repo".into(),
            dir_name: "repo".into(),
            clone_dir: clone_dir.clone(),
            error: None,
        };

        let result = sync_one_repo(&info, &meta, false, "rebase");

        assert_eq!(
            result.status,
            SyncRepoStatus::Ok,
            "clean repo should sync Ok"
        );
        assert!(
            result.error.is_none(),
            "clean sync should have no error; got: {:?}",
            result.error
        );
        let detail = result.detail.expect("detail should be present after sync");
        assert!(
            detail.contains("fast-forwarded") || detail.contains("up to date"),
            "detail should describe fast-forward; got: {:?}",
            detail
        );
    }

    #[test]
    fn test_sync_rerun_handles_new_conflict_on_clean_repo() {
        // Mixed-state scenario:
        //   Clone A: mid-rebase (conflict occurred), user resolves, rebase_continue succeeds → Ok/Resumed
        //   Clone B: clean but has a local commit that conflicts with origin → Paused after sync_one_repo
        //
        // This exercises the per-repo helpers used while resuming a sync.
        use std::process::Command as StdCommand;
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        // ----- Clone A setup -----
        let (clone_a, source, _ct_a, _st) = setup_clone_repo();

        // Introduce a conflict on clone A: origin gets "file_a.txt", local has same file.
        local_commit(&source, "file_a.txt", "origin side");
        git::fetch_remote_prune(&clone_a, "origin").unwrap();
        local_commit(&clone_a, "file_a.txt", "local side");

        let result = git::rebase_onto(&clone_a, "origin/main");
        assert!(result.is_err(), "clone A should conflict");
        assert_eq!(
            git::in_progress_op(&clone_a),
            Some(git::InProgressOp::Rebase),
            "clone A should be mid-rebase"
        );

        // Resolve clone A's conflict: write merged content and stage it.
        std::fs::write(clone_a.join("file_a.txt"), "resolved merged content").unwrap();
        let out = StdCommand::new("git")
            .args(["add", "file_a.txt"])
            .current_dir(&clone_a)
            .output()
            .unwrap();
        assert!(out.status.success(), "git add should succeed");

        // ----- Clone B setup -----
        let clone_b_tmp = tempfile::tempdir().unwrap();
        let clone_b = clone_b_tmp.path().join("repo_b");
        let out = StdCommand::new("git")
            .args(["clone", source.to_str().unwrap(), clone_b.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success(), "clone B should be created");

        for args in &[
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            // Check out a "feature" branch to match the workspace branch we'll use.
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
                .current_dir(&clone_b)
                .output()
                .unwrap();
            assert!(out.status.success(), "clone B setup: {:?}", args);
        }

        // Add another origin commit (different file) so clone_b's fetch has something.
        // Then add a conflicting local commit on clone_b.
        local_commit(&source, "file_b.txt", "origin side for b");
        git::fetch_remote_prune(&clone_b, "origin").unwrap();
        local_commit(&clone_b, "file_b.txt", "local side for b");

        // ----- Exercise -----
        // Exercise the same per-repo sequence used by run_live:
        //   1. Clone A is mid-rebase → call resume_repo.
        //   2. Clone B has no in-progress op → call sync_one_repo (which will hit conflict).

        // Step 1: resume clone A.
        let info_a = RepoInfo {
            identity: "test.local/user/repo_a".into(),
            dir_name: "repo_a".into(),
            clone_dir: clone_a.clone(),
            error: None,
        };
        let resume_result = resume_repo(&info_a, git::InProgressOp::Rebase, "feature");
        assert_eq!(
            resume_result.status,
            SyncRepoStatus::Ok,
            "clone A should resume successfully"
        );
        assert_eq!(
            resume_result.action, "rebase --continue",
            "the action should come from Git's in-progress operation"
        );
        assert_eq!(
            resume_result.strategy, "rebase",
            "the strategy should come from Git's in-progress operation"
        );
        assert!(
            git::in_progress_op(&clone_a).is_none(),
            "clone A should be clean after rebase_continue"
        );

        // Step 2: sync clone B via sync_one_repo — expect Paused (conflict).
        // Workspace branch is "feature" (clone_b is on feature branch).
        let meta = make_test_meta("feature");
        let info_b = RepoInfo {
            identity: "test.local/user/repo_b".into(),
            dir_name: "repo_b".into(),
            clone_dir: clone_b.clone(),
            error: None,
        };

        let result_b = sync_one_repo(&info_b, &meta, false, "rebase");
        assert_eq!(
            result_b.status,
            SyncRepoStatus::Paused,
            "clone B should be Paused after conflict"
        );
        assert_eq!(
            git::in_progress_op(&clone_b),
            Some(git::InProgressOp::Rebase),
            "clone B should be left mid-rebase"
        );
    }

    #[test]
    fn test_sync_one_repo_conflict_marks_paused_with_merge() {
        // sync_one_repo with "merge" strategy should classify a conflict as Paused
        // (not Failed) and leave MERGE_HEAD in place. Mirrors the rebase conflict
        // path but exercises the merge branch of Guard 6.
        //
        // Note: merge_continue at the git layer is fully covered by
        // git.rs::test_merge_continue. This test focuses on the sync_one_repo
        // wrapper's error classification for the merge strategy.
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();

        // setup_clone_repo leaves clone on "feature"; use that as workspace branch.
        let meta = make_test_meta("feature");

        // Cause a conflict: origin and local both modify the same file.
        local_commit(&source, "conflict.txt", "upstream merge side");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();
        local_commit(&clone_dir, "conflict.txt", "local merge side");

        let info = RepoInfo {
            identity: "test.local/user/repo".into(),
            dir_name: "repo".into(),
            clone_dir: clone_dir.clone(),
            error: None,
        };

        let result = sync_one_repo(&info, &meta, false, "merge");

        // Conflict via merge strategy → Paused (not Failed).
        assert_eq!(
            result.status,
            SyncRepoStatus::Paused,
            "merge conflict should produce Paused status"
        );
        assert!(
            clone_dir.join(".git/MERGE_HEAD").exists(),
            "MERGE_HEAD should exist — merge left mid-flight"
        );
        assert_eq!(
            git::in_progress_op(&clone_dir),
            Some(git::InProgressOp::Merge),
            "in_progress_op should report Merge"
        );
    }

    /// Guard 1: a RepoInfo with `error: Some(...)` must produce a Failed result
    /// without touching git at all.  Tests the first early-exit in sync_one_repo.
    #[test]
    fn test_sync_one_repo_error_repo_returns_failed() {
        // Construct a RepoInfo with a pre-existing config error.
        // The path is intentionally nonexistent — if Guard 1 is absent the
        // test would panic on any git call, making the guard observable.
        let info = RepoInfo {
            identity: "test.local/user/repo".into(),
            dir_name: "repo".into(),
            clone_dir: std::path::PathBuf::from("/nonexistent/path/repo"),
            error: Some("remote URL missing from config".into()),
        };
        let meta = make_test_meta("feature");

        let result = sync_one_repo(&info, &meta, false, "rebase");

        assert_eq!(
            result.status,
            SyncRepoStatus::Failed,
            "RepoInfo with error field must produce Failed status"
        );
        assert_eq!(
            result.error.as_deref(),
            Some("remote URL missing from config"),
            "error field must be propagated verbatim from RepoInfo"
        );
    }

    /// Guard 4: a dirty working tree (uncommitted tracked-file modifications) must
    /// produce a Failed result with an error mentioning "uncommitted changes".
    #[test]
    fn test_sync_one_repo_dirty_tree_returns_failed() {
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();

        // setup_clone_repo leaves clone on "feature"; use that as workspace branch.
        let meta = make_test_meta("feature");

        // Commit a tracked file in the clone so we can make it dirty later.
        local_commit(&clone_dir, "tracked.txt", "original content");

        // Add an upstream commit so origin/main is ahead of the clone.
        local_commit(&source, "upstream.txt", "upstream change");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();

        // Dirty the working tree without committing.
        std::fs::write(clone_dir.join("tracked.txt"), "modified content").unwrap();

        let changed = git::changed_file_count(&clone_dir).unwrap();
        assert!(changed > 0, "precondition: working tree must be dirty");

        let info = RepoInfo {
            identity: "test.local/user/repo".into(),
            dir_name: "repo".into(),
            clone_dir: clone_dir.clone(),
            error: None,
        };

        let result = sync_one_repo(&info, &meta, false, "rebase");

        assert_eq!(
            result.status,
            SyncRepoStatus::Failed,
            "dirty working tree must produce Failed status"
        );
        let err = result
            .error
            .expect("error field must be set for dirty-tree failure");
        assert!(
            err.contains("uncommitted changes"),
            "error must mention 'uncommitted changes'; got: {:?}",
            err
        );
    }

    #[test]
    fn continuation_with_clean_index_and_remaining_state_is_failed() {
        use wsp_core::testutil::setup_clone_repo;

        let (clone_dir, _source, _ct, _st) = setup_clone_repo();
        let merge_head = git::run(Some(&clone_dir), &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(clone_dir.join(".git/MERGE_HEAD"), merge_head).unwrap();

        assert_eq!(
            classify_continue_failure(&clone_dir),
            SyncRepoStatus::Failed,
            "operation state without unresolved index entries is a hard failure"
        );
    }

    #[test]
    fn discovery_only_includes_successful_repos() {
        let statuses = [
            SyncRepoStatus::Ok,
            SyncRepoStatus::Paused,
            SyncRepoStatus::Failed,
        ];
        assert_eq!(discoverable_repo_indices(&statuses), vec![0]);
    }

    #[test]
    fn fetch_failure_leaves_resolved_rebase_untouched() {
        use std::process::Command as StdCommand;
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();
        local_commit(&clone_dir, "conflict.txt", "local version");
        local_commit(&source, "conflict.txt", "upstream version");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();
        assert!(git::rebase_onto(&clone_dir, "origin/main").is_err());
        std::fs::write(clone_dir.join("conflict.txt"), "resolved").unwrap();
        let out = StdCommand::new("git")
            .args(["add", "conflict.txt"])
            .current_dir(&clone_dir)
            .output()
            .unwrap();
        assert!(out.status.success());

        let info = RepoInfo {
            identity: "test.local/user/repo".into(),
            dir_name: "repo".into(),
            clone_dir: clone_dir.clone(),
            error: None,
        };
        let result =
            sync_repo_after_fetch(&info, &make_test_meta("feature"), "rebase", Some("offline"));

        assert_eq!(result.status, SyncRepoStatus::Failed);
        assert!(result.error.unwrap().contains("offline"));
        assert_eq!(
            git::in_progress_op(&clone_dir),
            Some(git::InProgressOp::Rebase)
        );
    }

    #[test]
    fn resume_refuses_rebase_from_other_branch() {
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();
        local_commit(&clone_dir, "conflict.txt", "local version");
        local_commit(&source, "conflict.txt", "upstream version");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();
        assert!(git::rebase_onto(&clone_dir, "origin/main").is_err());

        let info = RepoInfo {
            identity: "test.local/user/repo".into(),
            dir_name: "repo".into(),
            clone_dir: clone_dir.clone(),
            error: None,
        };
        let result = sync_repo_after_fetch(&info, &make_test_meta("other"), "rebase", None);

        assert_eq!(result.status, SyncRepoStatus::Failed);
        assert!(result.error.unwrap().contains("on feature, expected other"));
        assert_eq!(
            git::in_progress_op(&clone_dir),
            Some(git::InProgressOp::Rebase)
        );
    }

    #[test]
    fn abort_requires_yes_when_noninteractive_and_work_exists() {
        let error = require_abort_confirmation(true, false, false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("pass --yes to confirm: wsp sync --abort --yes")
        );
        assert!(require_abort_confirmation(false, false, false).is_ok());
        assert!(require_abort_confirmation(true, true, false).is_ok());
    }

    #[test]
    fn abort_yes_is_accepted_only_with_abort() {
        cmd()
            .try_get_matches_from(["sync", "--abort", "--yes"])
            .expect("--abort --yes should parse");
        assert!(cmd().try_get_matches_from(["sync", "--yes"]).is_err());
    }

    #[test]
    fn run_abort_does_not_discard_work_without_yes_on_non_tty() {
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();
        local_commit(&clone_dir, "conflict.txt", "local version");
        local_commit(&source, "conflict.txt", "upstream version");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();
        assert!(git::rebase_onto(&clone_dir, "origin/main").is_err());

        let workspace_tmp = tempfile::tempdir().unwrap();
        let ws_dir = workspace_tmp.path().join("workspace");
        std::fs::create_dir(&ws_dir).unwrap();
        let repo_dir = ws_dir.join("repo");
        std::fs::rename(&clone_dir, &repo_dir).unwrap();

        let mut meta = make_test_meta("feature");
        let identity = "test.local/user/repo".to_string();
        meta.repos.insert(identity.clone(), None);
        meta.dirs.insert(identity, "repo".into());

        let error = match run_abort(&ws_dir, &meta, false) {
            Ok(_) => panic!("non-interactive abort without --yes must be rejected"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("pass --yes to confirm: wsp sync --abort --yes")
        );
        assert_eq!(
            git::in_progress_op(&repo_dir),
            Some(git::InProgressOp::Rebase),
            "the rejected abort must preserve the in-progress operation"
        );
    }

    /// Guard 6 (rebase path): a rebase conflict must produce Paused (not Failed)
    /// and leave the repo in mid-rebase state.  Mirrors
    /// test_sync_one_repo_conflict_marks_paused_with_merge for the rebase strategy.
    #[test]
    fn test_sync_one_repo_conflict_marks_paused_with_rebase() {
        use wsp_core::testutil::{local_commit, setup_clone_repo};

        let (clone_dir, source, _ct, _st) = setup_clone_repo();

        // setup_clone_repo leaves clone on "feature"; use that as workspace branch.
        let meta = make_test_meta("feature");

        // Cause a conflict: origin and local both modify the same file.
        local_commit(&source, "conflict.txt", "upstream rebase side");
        git::fetch_remote_prune(&clone_dir, "origin").unwrap();
        local_commit(&clone_dir, "conflict.txt", "local rebase side");

        let info = RepoInfo {
            identity: "test.local/user/repo".into(),
            dir_name: "repo".into(),
            clone_dir: clone_dir.clone(),
            error: None,
        };

        let result = sync_one_repo(&info, &meta, false, "rebase");

        // Conflict via rebase strategy → Paused (not Failed).
        assert_eq!(
            result.status,
            SyncRepoStatus::Paused,
            "rebase conflict should produce Paused status"
        );
        assert!(
            clone_dir.join(".git/rebase-merge").exists(),
            ".git/rebase-merge should exist — rebase left mid-flight"
        );
        assert_eq!(
            git::in_progress_op(&clone_dir),
            Some(git::InProgressOp::Rebase),
            "in_progress_op should report Rebase"
        );
    }
}
