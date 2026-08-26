use std::io::Write as _;

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::{self, Paths};
use wsp_core::git;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("rm")
        .visible_alias("remove")
        // Removes the workspace directory: step aside, then come back only if it
        // survived (a blocked removal leaves it in place).
        .add(crate::shellnav::ShellNav::vacates())
        .about("Remove a workspace")
        .long_about(
            "Remove a workspace.\n\n\
             Fetches from upstream, checks whether the workspace branch has been merged \
             (regular, squash, or rebase merge), and removes the workspace if safe. \
             Unmerged or pushed-but-unmerged branches block removal unless --force is used.\n\n\
             Workspaces are moved to a gc directory and can be restored with \
             `wsp recover <name>`. `wsp ls --removed` lists what is still restorable.",
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
        // Via invocation_dir: the wrapper vacates before running rm, so the
        // process cwd is the workspaces root rather than where the user was.
        let cwd = crate::shellcd::invocation_dir()?;
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
        // Capture workspace branch alongside PRs so dedup can distinguish a PR on
        // the current branch from a PR on the workspace branch for the same repo.
        let mut meta_branch_for_dedup: Option<String> = None;
        let open_prs: Vec<(String, String, u64, String)> =
            if cfg.pr_source.as_deref().is_some_and(|s| s != "false") {
                let ws_dir = workspace::dir(&paths.workspaces_dir, &name);
                workspace::load_metadata(&ws_dir)
                    .ok()
                    .map(|meta| {
                        meta_branch_for_dedup = Some(meta.branch.clone());
                        // Build (identity, branch) inputs. For each repo, include the
                        // current HEAD branch first (if it differs from meta.branch) so
                        // open PRs on whichever branch the user has checked out are also
                        // surfaced. meta.branch is always appended for every repo.
                        let mut inputs: Vec<(String, String)> = Vec::new();
                        for id in meta.repos.keys() {
                            if let Ok(dn) = meta.dir_name(id) {
                                let clone_dir = ws_dir.join(&dn);
                                let current = git::branch_current(&clone_dir).unwrap_or_default();
                                if !current.is_empty()
                                    && current != "HEAD"
                                    && current != meta.branch
                                    && git::validate_branch_name(&current).is_ok()
                                {
                                    inputs.push((id.clone(), current));
                                }
                            }
                            inputs.push((id.clone(), meta.branch.clone()));
                        }
                        // Same reasoning as `wsp st`: announce the network wait
                        // before taking it. Counted in repos, not `inputs`,
                        // which holds up to two branch queries per repo.
                        eprintln!(
                            "Fetching pull requests for {} repo{}...",
                            meta.repos.len(),
                            if meta.repos.len() == 1 { "" } else { "s" }
                        );
                        let pr_results = crate::pr::fetch_parallel(&inputs);
                        let mut seen = std::collections::HashSet::new();
                        pr_results
                            .into_iter()
                            .filter_map(|((id, branch), pr)| {
                                pr.filter(|p| p.state == "OPEN").and_then(|p| {
                                    if seen.insert((id.clone(), p.number)) {
                                        Some((id.clone(), branch.clone(), p.number, p.url.clone()))
                                    } else {
                                        None
                                    }
                                })
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
                for (id, _branch, number, url) in &open_prs {
                    eprintln!("  #{} {} ({})", number, id, url);
                }
            }
            // Show pushed-but-unmerged repos not already represented by an open PR
            // for the *same branch*. An open PR on the current branch must not
            // suppress a separate warning about the workspace branch being unmerged
            // (same identity, different branches).
            //
            // Blocker message formats (from workspace.rs):
            //   workspace branch: "{identity} (unmerged branch, but pushed to remote)"
            //   current branch:   "{identity} (current branch '{name}' is unmerged, ...)"
            if has_pushed_unmerged {
                let uncovered: Vec<&String> = blockers
                    .pushed_unmerged
                    .iter()
                    .filter(|msg| {
                        !open_prs.iter().any(|(id, pr_branch, _, _)| {
                            // Use " (" as a boundary so "acme/foo" doesn't suppress
                            // warnings for "acme/foo-bar".
                            let boundary = format!("{} (", id);
                            if !msg.starts_with(boundary.as_str()) {
                                return false;
                            }
                            // Same identity. Only suppress if the PR is for the same branch.
                            if msg.contains("current branch") {
                                // Branch name is embedded as "current branch '{name}'"
                                msg.contains(&format!("'{}'", pr_branch))
                            } else {
                                // Workspace branch message — match against meta.branch
                                meta_branch_for_dedup.as_deref() == Some(pr_branch.as_str())
                            }
                        })
                    })
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
    let gc_entry = workspace::remove(paths, &name, true)?;

    // A partial workspace is deleted outright rather than moved to gc. Saying
    // it is recoverable would be a lie the user only finds out about when they
    // try to recover it.
    let Some(gc_entry) = gc_entry else {
        return Ok(Output::Mutation(MutationOutput::new(format!(
            "Workspace {:?} removed. It had no metadata, so nothing was kept.",
            name
        ))));
    };

    // An absolute date, not "for 7 days": this line gets read again later, out
    // of a scrollback, when "from when" is no longer obvious. Both commands are
    // named so neither the restore nor the listing has to be guessed.
    // Derived from the removal just performed, so this and `wsp ls --removed`
    // cannot disagree about the deadline.
    let deadline =
        wsp_core::gc::expires_at(&gc_entry.trashed_at, wsp_core::gc::retention_days(paths));
    let window = match deadline {
        None => "recoverable indefinitely (gc disabled)".to_string(),
        Some(at) => format!(
            "recoverable until {}",
            at.with_timezone(&chrono::Local).format("%Y-%m-%d")
        ),
    };
    let hint = format!(
        "{} — `wsp recover {}` restores it, `wsp ls --removed` lists all",
        window, name
    );
    Ok(Output::Mutation(
        MutationOutput::new(format!("Workspace {:?} removed.", name)).with_hint(hint),
    ))
}
