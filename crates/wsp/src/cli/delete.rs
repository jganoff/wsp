use std::io::Write as _;

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::{self, Paths};
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("rm")
        .visible_alias("remove")
        .about("Remove a workspace")
        .long_about(
            "Remove a workspace.\n\n\
             Fetches from upstream, checks whether the workspace branch has been merged \
             (regular, squash, or rebase merge), and removes the workspace if safe. \
             Unmerged or pushed-but-unmerged branches block removal unless --force is used.\n\n\
             Workspaces are moved to a gc directory and can be recovered with `wsp recover`.",
        )
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .action(clap::ArgAction::SetTrue)
                .help("Remove even if repos have pending changes, unmerged branches, or workspace root has user content"),
        )
        .arg(
            Arg::new("yes")
                .short('y')
                .long("yes")
                .action(clap::ArgAction::SetTrue)
                .help("Skip confirmation prompt (for scripts and non-TTY callers)"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let force = matches.get_flag("force");
    let yes = matches.get_flag("yes") || force; // --force implies --yes

    let name = if let Some(n) = matches.get_one::<String>("workspace") {
        n.clone()
    } else {
        let cwd = std::env::current_dir()?;
        let ws_dir = workspace::detect(&cwd)?;
        let meta = workspace::load_metadata(&ws_dir)
            .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;
        meta.name
    };

    // Partial workspace: directory exists but no .wsp.yaml (wsp new interrupted).
    // All preconditions pass (there's nothing to check), but the content may not
    // have been created by wsp — confirm before deleting. This is --yes territory,
    // not --force: there's no safety invariant being overridden.
    if workspace::is_partial_workspace(paths, &name) {
        let ws_dir = workspace::dir(&paths.workspaces_dir, &name);
        eprintln!(
            "Warning: workspace {:?} has no .wsp.yaml (interrupted creation?).",
            name
        );
        eprintln!("  Directory: {}", ws_dir.display());
        if yes {
            // confirmed via --yes or --force
        } else if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            eprint!("  Remove it? [y/N]: ");
            std::io::stderr().flush()?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                anyhow::bail!("aborted");
            }
        } else {
            anyhow::bail!("pass --yes to confirm: wsp rm {:?} --yes", name);
        }
    }

    // Run all safety checks upfront (unless --force bypasses them all, or this is a
    // partial workspace which has no metadata to check and is handled by remove()).
    // This lets us error immediately on hard blockers and fold pushed-but-unmerged
    // branch warnings into the open-PR confirmation so the user only answers once.
    if !force && !workspace::is_partial_workspace(paths, &name) {
        let blockers = workspace::check_removal_blockers(paths, &name)?;

        // Hard blockers (uncommitted changes, linked worktrees, wrong-branch
        // unpushed commits, root content) and local-only unmerged branches cannot
        // be acknowledged via an open-PR prompt — error immediately.
        if !blockers.hard.is_empty() || !blockers.local_unmerged.is_empty() {
            let list = blockers
                .all_sorted()
                .iter()
                .map(|p| format!("\n  - {}", p))
                .collect::<String>();
            anyhow::bail!(
                "workspace {:?} has unsaved work ({}):{}\n\nUse --force to remove anyway",
                name,
                blockers.branch,
                list
            );
        }

        // Any remaining blockers are pushed-but-unmerged branches. The branch is
        // on the remote so the code isn't at risk of loss. Gather open PRs (if PR
        // source is configured) and show a single combined prompt covering both.
        let has_pushed_unmerged = !blockers.pushed_unmerged.is_empty();

        let cfg = config::Config::load_from(&paths.config_path).unwrap_or_default();
        let open_prs: Vec<(String, u64, String)> =
            if cfg.pr_source.as_deref().is_some_and(|s| s != "false") {
                let ws_dir = workspace::dir(&paths.workspaces_dir, &name);
                workspace::load_metadata(&ws_dir)
                    .ok()
                    .map(|meta| {
                        let inputs: Vec<(String, String)> = meta
                            .repos
                            .keys()
                            .map(|id| (id.clone(), meta.branch.clone()))
                            .collect();
                        let pr_results = crate::pr::fetch_parallel(&inputs);
                        meta.repos
                            .keys()
                            .zip(pr_results.iter())
                            .filter_map(|(id, pr)| {
                                pr.as_ref()
                                    .filter(|p| p.state == "OPEN")
                                    .map(|p| (id.clone(), p.number, p.url.clone()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            } else {
                vec![]
            };

        if !open_prs.is_empty() || has_pushed_unmerged {
            if !open_prs.is_empty() {
                eprintln!(
                    "Warning: {} open PR{} on this workspace:",
                    open_prs.len(),
                    if open_prs.len() == 1 { "" } else { "s" }
                );
                for (id, number, url) in &open_prs {
                    eprintln!("  #{} {} ({})", number, id, url);
                }
            }
            // Show pushed-but-unmerged repos not already represented by an open PR.
            // In single-repo workspaces this is usually empty (PR covers it). In
            // multi-repo workspaces some repos may have no PR source configured.
            if has_pushed_unmerged {
                let pr_ids: std::collections::HashSet<&str> =
                    open_prs.iter().map(|(id, _, _)| id.as_str()).collect();
                let uncovered: Vec<&String> = blockers
                    .pushed_unmerged
                    .iter()
                    .filter(|msg| !pr_ids.iter().any(|id| msg.starts_with(*id)))
                    .collect();
                if !uncovered.is_empty() {
                    eprintln!("Warning: workspace has a pushed-but-unmerged branch:");
                    for msg in uncovered {
                        eprintln!("  - {}", msg);
                    }
                }
            }

            if !yes {
                if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    eprint!("  Remove anyway? [y/N]: ");
                    std::io::stderr().flush()?;
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer)?;
                    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                        anyhow::bail!("aborted");
                    }
                } else {
                    anyhow::bail!(
                        "workspace has open PRs or unmerged branch; pass --yes to confirm: wsp rm {:?} --yes",
                        name
                    );
                }
            }
        }
    }

    eprintln!("Removing workspace {:?}...", name);
    // Safety checks were already run by check_removal_blockers() above (when !force).
    // Pass force=true so remove() skips redundant re-checking and re-fetching.
    workspace::remove(paths, &name, true)?;

    let cfg = wsp_core::config::Config::load_from(&paths.config_path).unwrap_or_default();
    let days = cfg
        .gc_retention_days
        .unwrap_or(wsp_core::gc::DEFAULT_RETENTION_DAYS);
    let hint = if days == 0 {
        "recoverable via `wsp recover` (gc disabled, kept indefinitely)".into()
    } else {
        format!(
            "recoverable via `wsp recover` for {} day{}",
            days,
            if days == 1 { "" } else { "s" }
        )
    };
    Ok(Output::Mutation(
        MutationOutput::new(format!("Workspace {:?} removed.", name)).with_hint(hint),
    ))
}
