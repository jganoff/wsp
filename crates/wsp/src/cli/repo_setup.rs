use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::giturl;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("setup")
        .about("Run setup commands for repos in the current workspace")
        .long_about(
            "Run setup commands for repos in the current workspace.\n\n\
             Commands are resolved from up to four layers (registry, template, repo \
             .wsp.yaml, workspace overrides) and concatenated. By default, exact \
             duplicates are removed before prompting (use --all to run every \
             occurrence).\n\n\
             Use `wsp repo setup-commands ls` to see the full list with provenance labels.",
        )
        .arg(
            Arg::new("repos")
                .num_args(0..)
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(
            Arg::new("all")
                .long("all")
                .action(clap::ArgAction::SetTrue)
                .help("Run all commands including duplicates across layers"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let filter: Vec<&String> = matches
        .get_many::<String>("repos")
        .map(|v| v.collect())
        .unwrap_or_default();
    let all = matches.get_flag("all");

    let cwd = crate::shellcd::invocation_dir()?;
    let ws_dir = workspace::detect(&cwd)?;
    let meta = workspace::load_metadata(&ws_dir)?;

    // Pre-resolve filter args to full identities so shortnames work.
    let cfg = wsp_core::config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;
    let identities: Vec<String> = cfg.repos.keys().cloned().collect();
    let resolved_filter: Vec<String> = filter
        .iter()
        .filter_map(|f| giturl::resolve(giturl::parse_repo_ref(f), &identities).ok())
        .collect();

    let mut ran = 0usize;
    let mut skipped = 0usize;

    for info in meta.repo_infos(&ws_dir) {
        if info.error.is_some() {
            continue;
        }
        if !filter.is_empty() && !resolved_filter.contains(&info.identity) {
            continue;
        }

        let resolved = {
            let r = wsp_core::setup_commands::resolve_for_repo(
                &cfg,
                None, // no template context for manual re-run
                Some(&meta),
                &info.identity,
                Some(&info.clone_dir),
            );
            if all { r } else { r.dedup() }
        };
        if resolved.is_empty() {
            continue;
        }

        match wsp_core::setup_runner::maybe_run_resolved(
            paths.data_dir(),
            &info.clone_dir,
            &info.identity,
            &resolved,
        ) {
            Ok(true) => ran += 1,
            Ok(false) => skipped += 1,
            Err(e) => eprintln!("warning: setup for {}: {}", info.identity, e),
        }
    }

    Ok(Output::Mutation(MutationOutput::new(format!(
        "Setup complete: {} ran, {} skipped.",
        ran, skipped
    ))))
}
